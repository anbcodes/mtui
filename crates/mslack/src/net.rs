// Slack Web API calls, run on a small pool of worker threads. Each worker
// keeps its own HTTP keep-alive connection. Results come back on a channel
// and wake the UI loop.

use mhttp::{self as http, Client};
use mtui::json::{self, Value};
use mtui::term::Waker;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

const WORKERS: usize = 4;

#[derive(Clone, Debug)]
pub enum Tag {
    Auth,
    Chans,
    Users,
    User(String),
    /// Initial load, poll for newer, refresh of the latest page, or older page.
    History(String, Fetch),
    Replies(String),
    Posted(String, Option<String>),
    Done(&'static str),
    Counts,
    /// A downloaded image; the bytes wait in `Net::blobs`.
    Image(String),
    Peek(String),
    Mark,
    /// Socket Mode state: Ok when connected, Err(reason) when the link dropped.
    Live,
    /// A pushed Events API event.
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fetch {
    Initial,
    Newer,
    Refresh,
    Older,
}

pub type Reply = (Tag, Result<Value, String>);

struct Auth {
    token: String,
    cookie: Option<String>,
    base: String,
}

type Job = Box<dyn FnOnce(&mut Client) + Send>;

#[derive(Clone)]
pub struct Net {
    auth: Arc<Auth>,
    tx: Sender<Reply>,
    waker: Waker,
    jobs: Sender<Job>,
    blobs: Arc<Mutex<HashMap<String, Result<Vec<u8>, String>>>>,
}

fn call_once(c: &mut Client, auth: &Auth, method: &str, params: &[(String, String)]) -> Result<Value, (String, u64)> {
    let bearer = format!("Bearer {}", auth.token);
    let cookie = auth.cookie.as_ref().map(|c| format!("d={}", c));
    let mut h = vec![("Authorization", bearer.as_str()), ("Content-Type", "application/x-www-form-urlencoded")];
    if let Some(c) = &cookie {
        h.push(("Cookie", c));
    }
    let res = c.request("POST", &format!("{}/{}", auth.base, method), &h, http::form(params).as_bytes()).map_err(|e| (e, 0))?;
    if res.status == 429 {
        let wait = res.header("retry-after").and_then(|v| v.parse().ok()).unwrap_or(5);
        return Err(("ratelimited".into(), wait));
    }
    let v = json::parse(&String::from_utf8_lossy(&res.body)).map_err(|e| (format!("HTTP {}: {}", res.status, e), 0))?;
    if !v.get("ok").bool() {
        return Err((v.get("error").opt_str().unwrap_or("request failed").to_string(), 0));
    }
    Ok(v)
}

/// Call, waiting out rate limits (up to a few times).
fn call_retry(c: &mut Client, auth: &Auth, method: &str, params: &[(String, String)]) -> Result<Value, String> {
    for _ in 0..4 {
        match call_once(c, auth, method, params) {
            Err((e, wait)) if e == "ratelimited" => std::thread::sleep(std::time::Duration::from_secs(wait.min(60))),
            r => return r.map_err(|e| e.0),
        }
    }
    Err("ratelimited".into())
}

pub fn base_url() -> String {
    std::env::var("SLACK_API_URL").unwrap_or_else(|_| "https://slack.com/api".into()).trim_end_matches('/').to_string()
}

fn own(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

impl Net {
    pub fn new(token: String, cookie: Option<String>, tx: Sender<Reply>, waker: Waker) -> Net {
        let (jobs, rx) = mpsc::channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = rx.clone();
            std::thread::spawn(move || {
                let mut client = Client::new();
                loop {
                    let job = rx.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(&mut client),
                        Err(_) => return,
                    }
                }
            });
        }
        Net { auth: Arc::new(Auth { token, cookie, base: base_url() }), tx, waker, jobs, blobs: Default::default() }
    }

    fn submit(&self, f: impl FnOnce(&mut Client, &Auth) -> Reply + Send + 'static) {
        let n = self.clone();
        let _ = self.jobs.send(Box::new(move |c| {
            let r = f(c, &n.auth);
            let _ = n.tx.send(r);
            n.waker.wake();
        }));
    }

    /// Download a Slack-hosted file (needs the token) for the image cache.
    pub fn fetch_image(&self, key: &str, url: &str) {
        let (key, url) = (key.to_string(), url.to_string());
        let blobs = self.blobs.clone();
        self.submit(move |c, auth| {
            let bearer = format!("Bearer {}", auth.token);
            let cookie = auth.cookie.as_ref().map(|c| format!("d={}", c));
            let mut h = vec![("Authorization", bearer.as_str())];
            if let Some(c) = &cookie {
                h.push(("Cookie", c));
            }
            let r = c.get_file(&url, &h, &["Authorization", "Cookie"], 12 << 20);
            blobs.lock().unwrap().insert(key.clone(), r);
            (Tag::Image(key), Ok(Value::Null))
        });
    }

    /// The bytes for a finished `Tag::Image`.
    pub fn take_image(&self, key: &str) -> Result<Vec<u8>, String> {
        self.blobs.lock().unwrap().remove(key).unwrap_or_else(|| Err("missing".into()))
    }

    pub fn call(&self, tag: Tag, method: &str, params: &[(&str, &str)]) {
        let (method, params) = (method.to_string(), own(params));
        self.submit(move |c, auth| (tag, call_retry(c, auth, &method, &params)));
    }

    /// Follow `next_cursor` pagination, concatenating the `key` arrays into
    /// one `{key: [...]}` result.
    pub fn call_all(&self, tag: Tag, method: &str, params: &[(&str, &str)], key: &'static str, max_pages: usize) {
        let (method, params) = (method.to_string(), own(params));
        self.submit(move |c, auth| {
            let mut all = Vec::new();
            let mut cursor = String::new();
            let mut res = Ok(());
            for _ in 0..max_pages {
                let mut p = params.clone();
                if !cursor.is_empty() {
                    p.push(("cursor".into(), cursor.clone()));
                }
                match call_retry(c, auth, &method, &p) {
                    Ok(v) => {
                        all.extend(v.get(key).arr().iter().cloned());
                        cursor = v.path("response_metadata.next_cursor").str().to_string();
                        if cursor.is_empty() {
                            break;
                        }
                    }
                    Err(e) => {
                        res = Err(e);
                        break;
                    }
                }
            }
            let r = match res {
                Err(e) if all.is_empty() => Err(e),
                _ => Ok(Value::Obj(vec![(key.to_string(), Value::Arr(all))])),
            };
            (tag, r)
        });
    }

    /// Upload files (external-upload flow) and share them in `channel`, with
    /// `text` as the comment. Replies as `Tag::Done("uploaded")`.
    pub fn upload(&self, channel: &str, thread: Option<&str>, text: &str, files: Vec<(String, Vec<u8>)>) {
        let (channel, thread, text) = (channel.to_string(), thread.map(str::to_string), text.to_string());
        self.submit(move |c, auth| {
            let r = (|| {
                let mut ids = Vec::new();
                for (name, data) in &files {
                    let len = data.len().to_string();
                    let v = call_retry(c, auth, "files.getUploadURLExternal", &own(&[("filename", name), ("length", &len)]))?;
                    let (url, id) = (v.get("upload_url").str().to_string(), v.get("file_id").str().to_string());
                    let res = c.request("POST", &url, &[("Content-Type", "application/octet-stream")], data)?;
                    if res.status >= 300 {
                        return Err(format!("upload failed (HTTP {})", res.status));
                    }
                    ids.push(format!("{{\"id\":{},\"title\":{}}}", json::quote(&id), json::quote(name)));
                }
                let list = format!("[{}]", ids.join(","));
                let mut p = own(&[("files", &list), ("channel_id", &channel)]);
                if !text.is_empty() {
                    p.push(("initial_comment".into(), text.clone()));
                }
                if let Some(t) = &thread {
                    p.push(("thread_ts".into(), t.clone()));
                }
                call_retry(c, auth, "files.completeUploadExternal", &p)
            })();
            (Tag::Done("uploaded"), r)
        });
    }
}
