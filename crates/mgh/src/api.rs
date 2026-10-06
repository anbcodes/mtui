// GitHub REST calls on a small pool of worker threads. Each worker keeps its
// own keep-alive connection. Results come back on a channel and wake the UI
// loop. List calls send If-None-Match so unchanged lists cost a 304.

use mhttp::Client;
use mtui::json::{self, Value};
use mtui::term::Waker;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

const WORKERS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    User,
    /// A tab's list.
    List(usize),
    Repos,
    /// Parts of the open item; the number is the detail generation.
    Issue(u64),
    Comments(u64),
    Pull(u64),
    Reviews(u64),
    Checks(u64),
    Diff(u64),
    Act(&'static str),
}

pub struct Resp {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Value,
    pub text: String,
}

pub type Reply = (Tag, Result<Resp, String>);

struct Auth {
    token: String,
    base: String,
}

type Job = Box<dyn FnOnce(&mut Client) + Send>;

#[derive(Clone)]
pub struct Net {
    auth: Arc<Auth>,
    tx: Sender<Reply>,
    waker: Waker,
    jobs: Sender<Job>,
}

pub fn base_url() -> String {
    std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".into()).trim_end_matches('/').to_string()
}

pub const JSON: &str = "application/vnd.github+json";
pub const DIFF: &str = "application/vnd.github.diff";

pub struct Req {
    pub method: &'static str,
    pub path: String,
    pub body: Option<String>,
    pub accept: &'static str,
    pub etag: Option<String>,
}

impl Req {
    pub fn get(path: impl Into<String>) -> Req {
        Req { method: "GET", path: path.into(), body: None, accept: JSON, etag: None }
    }
    pub fn send(method: &'static str, path: impl Into<String>, body: String) -> Req {
        Req { method, path: path.into(), body: Some(body), accept: JSON, etag: None }
    }
}

fn run(c: &mut Client, a: &Auth, r: &Req) -> Result<Resp, String> {
    let bearer = format!("Bearer {}", a.token);
    let mut h = vec![("Authorization", bearer.as_str()), ("Accept", r.accept), ("X-GitHub-Api-Version", "2022-11-28")];
    if r.body.is_some() {
        h.push(("Content-Type", "application/json"));
    }
    if let Some(e) = &r.etag {
        h.push(("If-None-Match", e));
    }
    let res = c.request(r.method, &format!("{}{}", a.base, r.path), &h, r.body.as_deref().unwrap_or("").as_bytes())?;
    let text = String::from_utf8_lossy(&res.body).into_owned();
    let body = if text.trim_start().starts_with(['{', '[']) { json::parse(&text).unwrap_or(Value::Null) } else { Value::Null };
    if res.status >= 400 {
        let m = body.get("message").opt_str().map(String::from).unwrap_or_else(|| format!("HTTP {}", res.status));
        let first_err = body.get("errors").arr().first().map(|e| e.opt_str().map(String::from).unwrap_or_else(|| e.get("message").str().to_string())).filter(|s| !s.is_empty());
        return Err(match first_err {
            Some(e) => format!("{}: {}", m, e),
            None => m,
        });
    }
    Ok(Resp { status: res.status, etag: res.header("etag").map(String::from), body, text })
}

impl Net {
    pub fn new(token: String, tx: Sender<Reply>, waker: Waker) -> Net {
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
        Net { auth: Arc::new(Auth { token, base: base_url() }), tx, waker, jobs }
    }

    pub fn call(&self, tag: Tag, req: Req) {
        let n = self.clone();
        let _ = self.jobs.send(Box::new(move |c| {
            let r = run(c, &n.auth, &req);
            let _ = n.tx.send((tag, r));
            n.waker.wake();
        }));
    }
}
