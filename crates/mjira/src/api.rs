// Jira REST calls on a small pool of worker threads, each with its own
// keep-alive connection. Results come back on a channel and wake the UI loop.
// Jira Cloud speaks API v3 (documents are ADF, users have account ids, search
// pages by token); Server and Data Center speak v2 (wiki text, user names,
// search pages by offset). Everything else is the same.

use mhttp::Client;
use mtui::json::{self, Value};
use mtui::term::Waker;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

const WORKERS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flavor {
    Cloud,
    Server,
}

/// Why transitions were asked for: to pick one, or to move a card to a column.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    Pick(String),
    Move(String, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Me,
    /// A tab's list: which, the list's generation (a reply to an older query
    /// is dropped), and whether it extends the list with the next page.
    List(usize, u64, bool),
    Projects,
    Filters,
    /// The statuses of a project, in workflow order (the board's columns).
    Statuses(String),
    /// Parts of the open issue; the number is the detail generation.
    Issue(u64),
    Comments(u64),
    Transitions(Why),
    /// Assignable users of an issue.
    Users(String),
    Priorities(String),
    /// A project's issue types, for creating an issue.
    Types,
    Created,
    /// Image bytes for the gallery, by key.
    Image(String),
    Act(&'static str),
}

pub struct Resp {
    pub body: Value,
    pub bytes: Vec<u8>,
}

pub type Reply = (Tag, Result<Resp, String>);

pub struct Auth {
    header: String,
    pub base: String,
    pub flavor: Flavor,
}

pub struct Req {
    pub method: &'static str,
    pub path: String,
    pub body: Option<String>,
}

impl Req {
    pub fn get(path: impl Into<String>) -> Req {
        Req { method: "GET", path: path.into(), body: None }
    }
    pub fn send(method: &'static str, path: impl Into<String>, body: String) -> Req {
        Req { method, path: path.into(), body: Some(body) }
    }
}

pub const FIELDS: &str = "summary,status,issuetype,priority,assignee,reporter,updated,project,parent,labels";

/// A JQL search; `page` is the cursor `done()` returned for the previous page.
pub fn search(f: Flavor, jql: &str, page: Option<&str>, max: usize) -> Req {
    let q = format!("jql={}&fields={}&maxResults={}", mhttp::urlencode(jql), FIELDS, max);
    match (f, page) {
        (Flavor::Cloud, None) => Req::get(format!("/search/jql?{}", q)),
        (Flavor::Cloud, Some(t)) => Req::get(format!("/search/jql?{}&nextPageToken={}", q, mhttp::urlencode(t))),
        (Flavor::Server, p) => Req::get(format!("/search?{}&startAt={}", q, p.unwrap_or("0"))),
    }
}

/// The cursor for the page after this search result, if there is one.
pub fn next_page(f: Flavor, body: &Value) -> Option<String> {
    match f {
        Flavor::Cloud => body.get("nextPageToken").opt_str().map(String::from),
        Flavor::Server => {
            let next = body.get("startAt").num() as usize + body.get("issues").arr().len();
            (next < body.get("total").num() as usize && !body.get("issues").arr().is_empty()).then(|| next.to_string())
        }
    }
}

type Job = Box<dyn FnOnce(&mut Client) + Send>;

#[derive(Clone)]
pub struct Net {
    auth: Arc<Auth>,
    tx: Sender<Reply>,
    waker: Waker,
    jobs: Sender<Job>,
}

/// Jira's error bodies: `{"errorMessages": [...], "errors": {"field": "why"}}`.
fn error_text(body: &Value, status: u16) -> String {
    let mut parts: Vec<String> = body.get("errorMessages").arr().iter().map(|m| m.str().to_string()).filter(|s| !s.is_empty()).collect();
    if let Value::Obj(m) = body.get("errors") {
        parts.extend(m.iter().map(|(k, v)| format!("{}: {}", k, v.str())));
    }
    if parts.is_empty() {
        if let Some(m) = body.get("message").opt_str() {
            parts.push(m.to_string());
        }
    }
    if parts.is_empty() {
        return match status {
            401 => "HTTP 401: check the email and API token".into(),
            403 => "HTTP 403: not allowed".into(),
            404 => "HTTP 404: not found (or you can't see it)".into(),
            s => format!("HTTP {}", s),
        };
    }
    parts.join("; ")
}

fn run(c: &mut Client, a: &Auth, r: &Req) -> Result<Resp, String> {
    let v = if a.flavor == Flavor::Cloud { 3 } else { 2 };
    let mut h = vec![("Authorization", a.header.as_str()), ("Accept", "application/json")];
    if r.body.is_some() {
        h.push(("Content-Type", "application/json"));
    }
    let res = c.request(r.method, &format!("{}/rest/api/{}{}", a.base, v, r.path), &h, r.body.as_deref().unwrap_or("").as_bytes())?;
    let text = String::from_utf8_lossy(&res.body);
    let body = if text.trim_start().starts_with(['{', '[']) { json::parse(&text).unwrap_or(Value::Null) } else { Value::Null };
    if res.status >= 400 {
        return Err(error_text(&body, res.status));
    }
    Ok(Resp { body, bytes: Vec::new() })
}

impl Net {
    pub fn new(base: String, flavor: Flavor, auth_header: String, tx: Sender<Reply>, waker: Waker) -> Net {
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
        Net { auth: Arc::new(Auth { header: auth_header, base: base.trim_end_matches('/').to_string(), flavor }), tx, waker, jobs }
    }

    pub fn flavor(&self) -> Flavor {
        self.auth.flavor
    }

    pub fn base(&self) -> &str {
        &self.auth.base
    }

    fn submit(&self, f: impl FnOnce(&mut Client, &Auth) -> Reply + Send + 'static) {
        let n = self.clone();
        let _ = self.jobs.send(Box::new(move |c| {
            let r = f(c, &n.auth);
            let _ = n.tx.send(r);
            n.waker.wake();
        }));
    }

    pub fn call(&self, tag: Tag, req: Req) {
        self.submit(move |c, auth| (tag, run(c, auth, &req)));
    }

    /// An image: attachments of this site need the credentials, which are
    /// dropped again if the file redirects to another host.
    pub fn fetch_image(&self, key: &str, url: &str) {
        let (key, url) = (key.to_string(), url.to_string());
        self.submit(move |c, a| {
            let mine = url.starts_with(&a.base);
            let h = [("Authorization", if mine { a.header.as_str() } else { "" }), ("Accept", "image/png,image/jpeg,*/*")];
            let h: Vec<(&str, &str)> = h.iter().copied().filter(|(_, v)| !v.is_empty()).collect();
            let r = c.get_file(&url, &h, &["Authorization"], 12 << 20);
            (Tag::Image(key), r.map(|bytes| Resp { body: Value::Null, bytes }))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches() {
        let r = search(Flavor::Cloud, "project = A", Some("tok en"), 50);
        assert!(r.path.starts_with("/search/jql?jql=project%20%3D%20A&fields=") && r.path.ends_with("&nextPageToken=tok%20en"));
        let r = search(Flavor::Server, "x", Some("100"), 50);
        assert!(r.path.starts_with("/search?jql=x") && r.path.ends_with("&startAt=100"));
        let b = json::parse(r#"{"startAt":0,"total":120,"issues":[{},{}]}"#).unwrap();
        assert_eq!(next_page(Flavor::Server, &b).as_deref(), Some("2"));
        assert_eq!(next_page(Flavor::Cloud, &json::parse(r#"{"issues":[]}"#).unwrap()), None);
    }

    #[test]
    fn errors() {
        let b = json::parse(r#"{"errorMessages":["No such issue"],"errors":{"summary":"required"}}"#).unwrap();
        assert_eq!(error_text(&b, 400), "No such issue; summary: required");
        assert!(error_text(&Value::Null, 401).contains("API token"));
    }
}
