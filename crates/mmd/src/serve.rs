// The live preview: a tiny HTTP server on localhost that serves the rendered
// page, pushes `reload` and `scroll` events to the browser (server-sent
// events), serves images next to the document, and lets the page report a
// double-clicked source line back to the editor.

use crate::md::{self, Doc, Options};
use crate::page;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

struct State {
    src: String,
    doc: Doc,
    title: String,
    version: u64,
    scroll: usize,
    scroll_seq: u64,
    goto: Option<usize>,
    clients: usize,
}

struct Shared {
    st: Mutex<State>,
    cv: Condvar,
    closed: AtomicBool,
    base: Mutex<Option<PathBuf>>,
    opts: Options,
}

pub struct Server {
    shared: Arc<Shared>,
    port: u16,
}

impl Server {
    /// Listen on 127.0.0.1 (`port` 0 picks a free one). `base` is where
    /// relative images are served from.
    pub fn start(port: u16, base: Option<PathBuf>, title: &str, opts: Options) -> std::io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let doc = md::render("", &opts);
        let shared = Arc::new(Shared { st: Mutex::new(State { src: String::new(), doc, title: title.into(), version: 1, scroll: 0, scroll_seq: 0, goto: None, clients: 0 }), cv: Condvar::new(), closed: AtomicBool::new(false), base: Mutex::new(base), opts });
        let sh = shared.clone();
        std::thread::spawn(move || {
            while !sh.closed.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let sh = sh.clone();
                        std::thread::spawn(move || {
                            let _ = s.set_nonblocking(false);
                            let _ = handle(s, &sh);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(40)),
                }
            }
        });
        Ok(Server { shared, port })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Replace the document. Browsers reload only if the text changed.
    pub fn set_source(&self, text: &str) {
        let mut st = self.shared.st.lock().unwrap();
        if st.src == text {
            return;
        }
        st.src = text.to_string();
        st.doc = md::render(text, &self.shared.opts);
        st.version += 1;
        self.shared.cv.notify_all();
    }

    pub fn set_title(&self, t: &str) {
        let mut st = self.shared.st.lock().unwrap();
        if st.title != t {
            st.title = t.into();
            st.version += 1;
            self.shared.cv.notify_all();
        }
    }

    pub fn set_base(&self, base: Option<PathBuf>) {
        *self.shared.base.lock().unwrap() = base;
    }

    /// Scroll browsers so 1-based source `line` is in view.
    pub fn scroll_to(&self, line: usize) {
        let mut st = self.shared.st.lock().unwrap();
        if st.scroll != line {
            st.scroll = line;
            st.scroll_seq += 1;
            self.shared.cv.notify_all();
        }
    }

    /// A line the user double-clicked in the browser, once.
    pub fn take_goto(&self) -> Option<usize> {
        self.shared.st.lock().unwrap().goto.take()
    }

    /// Browsers currently connected.
    pub fn clients(&self) -> usize {
        self.shared.st.lock().unwrap().clients
    }

    /// Follow a file on disk: re-render whenever it changes.
    pub fn watch(&self, path: PathBuf) {
        let sh = self.shared.clone();
        std::thread::spawn(move || {
            let mut last = None;
            while !sh.closed.load(Ordering::Relaxed) {
                let meta = std::fs::metadata(&path).ok().map(|m| (m.modified().ok(), m.len()));
                if meta != last {
                    last = meta;
                    if let Ok(t) = std::fs::read_to_string(&path) {
                        let mut st = sh.st.lock().unwrap();
                        if st.src != t {
                            st.src = t.clone();
                            st.doc = md::render(&t, &sh.opts);
                            st.version += 1;
                            sh.cv.notify_all();
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        });
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
    }
}

/// Open a URL in the default browser.
pub fn open_browser(url: &str) -> Result<(), String> {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("{}: {}", cmd, e))
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    write!(s, "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", code, reason, ctype, body.len())?;
    s.write_all(body)
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                o.push(v);
                i += 3;
                continue;
            }
        }
        o.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

fn ctype(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "css" => "text/css",
        "html" | "htm" => "text/html; charset=utf-8",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn handle(mut s: TcpStream, sh: &Arc<Shared>) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("/").to_string());
    let mut host_ok = false;
    let mut clen = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        let l = h.to_lowercase();
        if let Some(v) = l.strip_prefix("host:") {
            let v = v.trim();
            host_ok = v.starts_with("127.0.0.1") || v.starts_with("localhost") || v.starts_with("[::1]");
        }
        if let Some(v) = l.strip_prefix("content-length:") {
            clen = v.trim().parse().unwrap_or(0);
        }
    }
    if clen > 0 {
        let mut sink = vec![0u8; clen.min(1 << 16)];
        let _ = r.read(&mut sink);
    }
    if !host_ok {
        return respond(&mut s, 403, "text/plain", b"bad host");
    }
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{}=", k)).map(String::from));
    match (method.as_str(), path) {
        ("GET", "/") => {
            let st = sh.st.lock().unwrap();
            let html = page::page(&st.doc, if st.title.is_empty() { st.doc.title.as_deref().unwrap_or("Preview") } else { &st.title }, true, st.version);
            drop(st);
            respond(&mut s, 200, "text/html; charset=utf-8", html.as_bytes())
        }
        ("POST", "/goto") => {
            if let Some(l) = param("line").and_then(|v| v.parse::<usize>().ok()) {
                sh.st.lock().unwrap().goto = Some(l);
            }
            respond(&mut s, 204, "text/plain", b"")
        }
        ("GET", "/events") => events(s, sh, param("v").and_then(|v| v.parse().ok()).unwrap_or(0)),
        ("GET", p) => {
            let Some(base) = sh.base.lock().unwrap().clone() else { return respond(&mut s, 404, "text/plain", b"not found") };
            let rel = decode(p.trim_start_matches('/'));
            let full = base.join(&rel);
            let (Ok(full), Ok(base)) = (full.canonicalize(), base.canonicalize()) else { return respond(&mut s, 404, "text/plain", b"not found") };
            if !full.starts_with(&base) || !full.is_file() {
                return respond(&mut s, 404, "text/plain", b"not found");
            }
            match std::fs::read(&full) {
                Ok(b) => respond(&mut s, 200, ctype(&full), &b),
                Err(_) => respond(&mut s, 404, "text/plain", b"not found"),
            }
        }
        _ => respond(&mut s, 400, "text/plain", b"bad request"),
    }
}

fn events(mut s: TcpStream, sh: &Arc<Shared>, page_version: u64) -> std::io::Result<()> {
    s.set_read_timeout(None)?;
    write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n: hello\n\n")?;
    s.flush()?;
    let mut st = sh.st.lock().unwrap();
    st.clients += 1;
    let (mut seen_v, mut seen_s) = (page_version, 0);
    let mut last_write = Instant::now();
    let out = loop {
        if sh.closed.load(Ordering::Relaxed) {
            break Ok(());
        }
        let mut msg = String::new();
        if st.version != seen_v {
            seen_v = st.version;
            msg += &format!("event: reload\ndata: {}\n\n", seen_v);
            // the page re-fetches and then needs to be put back where it was
            seen_s = 0;
        }
        if st.scroll_seq != seen_s && st.scroll > 0 {
            seen_s = st.scroll_seq;
            msg += &format!("event: scroll\ndata: {}\n\n", st.scroll);
        }
        if msg.is_empty() && last_write.elapsed() > Duration::from_secs(15) {
            msg = ": ping\n\n".into();
        }
        if !msg.is_empty() {
            drop(st);
            let w = s.write_all(msg.as_bytes()).and_then(|_| s.flush());
            st = sh.st.lock().unwrap();
            last_write = Instant::now();
            if let Err(e) = w {
                break Err(e);
            }
            continue;
        }
        st = sh.cv.wait_timeout(st, Duration::from_secs(5)).unwrap().0;
    };
    st.clients -= 1;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", path).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_pages_events_and_files() {
        let dir = std::env::temp_dir().join(format!("mmd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"PNG").unwrap();
        let srv = Server::start(0, Some(dir.clone()), "T", Options::default()).unwrap();
        srv.set_source("# Hello\n\ntext");
        let page = get(srv.port, "/");
        assert!(page.contains("<h1 id=\"hello\"") && page.contains("<script>") && page.contains("content=\"2\""));
        assert!(get(srv.port, "/a.png").starts_with("HTTP/1.1 200") && get(srv.port, "/a.png").ends_with("PNG"));
        assert!(get(srv.port, "/../etc/passwd").starts_with("HTTP/1.1 404"));
        assert!(get(srv.port, "/%2e%2e/%2e%2e/etc/passwd").starts_with("HTTP/1.1 404"));
        // an event stream for a stale page gets a reload at once, then scroll events
        let mut ev = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        write!(ev, "GET /events?v=1 HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        ev.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        srv.scroll_to(7);
        std::thread::sleep(Duration::from_millis(300));
        let mut buf = [0u8; 2048];
        let n = ev.read(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(text.contains("event: reload") && text.contains("event: scroll\ndata: 7"), "{}", text);
        assert_eq!(srv.clients(), 1);
        // a double click reports its line
        let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        write!(s, "POST /goto?line=3 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n").unwrap();
        let mut o = String::new();
        s.read_to_string(&mut o).unwrap();
        assert_eq!(srv.take_goto(), Some(3));
        assert_eq!(srv.take_goto(), None);
        // other hosts are refused
        let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        write!(s, "GET / HTTP/1.1\r\nHost: evil.test\r\n\r\n").unwrap();
        let mut o = String::new();
        s.read_to_string(&mut o).unwrap();
        assert!(o.starts_with("HTTP/1.1 403"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
