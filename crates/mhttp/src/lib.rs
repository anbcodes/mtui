// Shared by mslack and mgh. A small HTTP/1.1 client over rustls (or plain TCP for http:// test
// servers). Keeps one connection alive per client and reuses it.

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(b),
            Stream::Tls(s) => s.read(b),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(b),
            Stream::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, String> {
        let (tls, rest) = match s.split_once("://") {
            Some(("https" | "wss", r)) => (true, r),
            Some(("http" | "ws", r)) => (false, r),
            _ => return Err(format!("unsupported URL: {}", s)),
        };
        let (hostport, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().map_err(|_| format!("bad port in {}", s))?),
            None => (hostport, if tls { 443 } else { 80 }),
        };
        Ok(Url { tls, host: host.to_string(), port, path: path.to_string() })
    }

    fn key(&self) -> String {
        format!("{}:{}:{}", self.tls, self.host, self.port)
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, String> {
    static CFG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(c);
        }
        if roots.is_empty() {
            return Err("no CA certificates found (set SSL_CERT_FILE)".into());
        }
        let cfg = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Arc::new(cfg))
    })
    .clone()
}

pub fn connect(u: &Url, timeout: Duration) -> Result<Stream, String> {
    let sock = TcpStream::connect((u.host.as_str(), u.port)).map_err(|e| format!("{}: {}", u.host, e))?;
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let _ = sock.set_nodelay(true);
    if !u.tls {
        return Ok(Stream::Plain(sock));
    }
    let name = ServerName::try_from(u.host.clone()).map_err(|e| e.to_string())?;
    let conn = ClientConnection::new(tls_config()?, name).map_err(|e| e.to_string())?;
    Ok(Stream::Tls(Box::new(StreamOwned::new(conn, sock))))
}

/// Read a response head: status code and lower-cased headers.
pub fn read_head(r: &mut impl BufRead) -> io::Result<(u16, Vec<(String, String)>)> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
    }
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::other(format!("bad status line: {}", line.trim())))?;
    let mut headers = Vec::new();
    loop {
        line.clear();
        r.read_line(&mut line)?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Ok((status, headers))
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
pub struct Client {
    conn: Option<(String, BufReader<Stream>)>,
}

const TIMEOUT: Duration = Duration::from_secs(30);

impl Client {
    pub fn new() -> Client {
        Client::default()
    }

    pub fn request(&mut self, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
        let u = Url::parse(url)?;
        // A reused connection may have been closed by the server while
        // idle; retry once on a fresh one.
        let reused = matches!(&self.conn, Some((k, _)) if *k == u.key());
        match self.try_request(&u, method, headers, body) {
            Err(_) if reused => {
                self.conn = None;
                self.try_request(&u, method, headers, body)
            }
            r => r,
        }
        .map_err(|e| {
            self.conn = None;
            e
        })
    }

    fn try_request(&mut self, u: &Url, method: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
        if !matches!(&self.conn, Some((k, _)) if *k == u.key()) {
            self.conn = Some((u.key(), BufReader::new(connect(u, TIMEOUT)?)));
        }
        let r = &mut self.conn.as_mut().unwrap().1;
        let mut req = format!("{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: mhttp/{}\r\nContent-Length: {}\r\n", method, u.path, u.host, env!("CARGO_PKG_VERSION"), body.len());
        for (k, v) in headers {
            req += &format!("{}: {}\r\n", k, v);
        }
        req += "\r\n";
        let mut buf = req.into_bytes();
        buf.extend_from_slice(body);
        let s = r.get_mut();
        s.write_all(&buf).and_then(|_| s.flush()).map_err(|e| e.to_string())?;

        let (status, hdrs) = read_head(r).map_err(|e| e.to_string())?;
        let get = |k: &str| hdrs.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
        let mut body = Vec::new();
        let mut keep = get("connection").map_or(true, |v| !v.eq_ignore_ascii_case("close"));
        if get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
            let mut line = String::new();
            loop {
                line.clear();
                r.read_line(&mut line).map_err(|e| e.to_string())?;
                let n = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16).map_err(|_| "bad chunk size".to_string())?;
                if n == 0 {
                    // trailers
                    loop {
                        line.clear();
                        r.read_line(&mut line).map_err(|e| e.to_string())?;
                        if line.trim().is_empty() {
                            break;
                        }
                    }
                    break;
                }
                let at = body.len();
                body.resize(at + n, 0);
                r.read_exact(&mut body[at..]).map_err(|e| e.to_string())?;
                line.clear();
                r.read_line(&mut line).map_err(|e| e.to_string())?;
            }
        } else if let Some(n) = get("content-length").and_then(|v| v.parse::<usize>().ok()) {
            body.resize(n, 0);
            r.read_exact(&mut body).map_err(|e| e.to_string())?;
        } else if method != "HEAD" && status != 204 && status != 304 {
            r.read_to_end(&mut body).map_err(|e| e.to_string())?;
            keep = false;
        }
        if !keep {
            self.conn = None;
        }
        Ok(Response { status, headers: hdrs, body })
    }
}

impl Client {
    /// GET a file, following redirects. Headers named in `private` (e.g.
    /// Authorization, Cookie) are dropped once the redirect leaves the host.
    pub fn get_file(&mut self, url: &str, headers: &[(&str, &str)], private: &[&str], limit: usize) -> Result<Vec<u8>, String> {
        let mut url = url.to_string();
        let origin = Url::parse(&url)?.host;
        for _ in 0..5 {
            let u = Url::parse(&url)?;
            let same = u.host == origin;
            let h: Vec<(&str, &str)> = headers.iter().copied().filter(|(k, _)| same || !private.iter().any(|p| p.eq_ignore_ascii_case(k))).collect();
            let r = self.request("GET", &url, &h, b"")?;
            match r.status {
                200 => return if r.body.len() > limit { Err("file too large".into()) } else { Ok(r.body) },
                301 | 302 | 303 | 307 | 308 => {
                    let loc = r.header("location").ok_or("redirect without location")?;
                    url = if loc.contains("://") { loc.to_string() } else if let Some(p) = loc.strip_prefix('/') { format!("{}://{}:{}/{}", if u.tls { "https" } else { "http" }, u.host, u.port, p) } else { return Err("bad redirect".into()) };
                }
                s => return Err(format!("HTTP {}", s)),
            }
        }
        Err("too many redirects".into())
    }
}

/// GET `url` and hand each line of the response body to `on_line` (for
/// server-sent events). Stops when it returns false, the server closes the
/// connection, or no data arrives for `timeout`. Handles chunked bodies.
pub fn stream_lines(url: &str, headers: &[(&str, &str)], timeout: Duration, mut on_line: impl FnMut(&str) -> bool) -> Result<(), String> {
    let u = Url::parse(url)?;
    let mut r = BufReader::new(connect(&u, timeout)?);
    let mut req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: mhttp/{}\r\nAccept: text/event-stream\r\nCache-Control: no-cache\r\n", u.path, u.host, env!("CARGO_PKG_VERSION"));
    for (k, v) in headers {
        req += &format!("{}: {}\r\n", k, v);
    }
    req += "\r\n";
    let s = r.get_mut();
    s.write_all(req.as_bytes()).and_then(|_| s.flush()).map_err(|e| e.to_string())?;
    let (status, hdrs) = read_head(&mut r).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("HTTP {}", status));
    }
    let chunked = hdrs.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let mut pending: Vec<u8> = Vec::new();
    let mut line = String::new();
    loop {
        if chunked {
            line.clear();
            r.read_line(&mut line).map_err(|e| e.to_string())?;
            let n = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16).map_err(|_| "bad chunk size".to_string())?;
            if n == 0 {
                return Ok(());
            }
            let at = pending.len();
            pending.resize(at + n, 0);
            r.read_exact(&mut pending[at..]).map_err(|e| e.to_string())?;
            line.clear();
            r.read_line(&mut line).map_err(|e| e.to_string())?;
        } else {
            let n = r.read_until(b'\n', &mut pending).map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok(());
            }
        }
        while let Some(i) = pending.iter().position(|&b| b == b'\n') {
            let l: Vec<u8> = pending.drain(..=i).collect();
            if !on_line(String::from_utf8_lossy(&l).trim_end_matches(['\r', '\n'])) {
                return Ok(());
            }
        }
    }
}

pub fn urlencode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{:02X}", b)),
        }
    }
    o
}

pub fn form(params: &[(String, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v))).collect::<Vec<_>>().join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = Url::parse("wss://wss-primary.slack.com/link/?ticket=abc").unwrap();
        assert!(u.tls && u.port == 443 && u.host == "wss-primary.slack.com" && u.path == "/link/?ticket=abc");
        let u = Url::parse("http://127.0.0.1:8765").unwrap();
        assert!(!u.tls && u.port == 8765 && u.path == "/");
        assert_eq!(urlencode("a b&c=é"), "a%20b%26c%3D%C3%A9");
    }
}
