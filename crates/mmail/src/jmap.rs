// JMAP (RFC 8620/8621), the protocol Fastmail's own web client speaks, so the
// mailbox here is the very same one. Requests run on a few worker threads,
// each with its own keep-alive connection; results come back on a channel and
// wake the UI. A push thread holds an EventSource connection open and reports
// every state change the server makes, whoever made it.

use mhttp::Client;
use mtui::json::{self, quote, Value};
use mtui::term::Waker;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WORKERS: usize = 4;
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";

pub const DEFAULT_SESSION: &str = "https://api.fastmail.com/jmap/session";

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub event_url: String,
    pub account: String,
    pub username: String,
    /// Capabilities the token has; asking for one it lacks fails the whole request.
    pub caps: Vec<String>,
}

pub fn fetch_session(url: &str, token: &str) -> Result<Session, String> {
    let bearer = format!("Bearer {}", token);
    let mut c = Client::new();
    let bytes = c.get_file(url, &[("Authorization", &bearer), ("Accept", "application/json")], &["Authorization"], 1 << 20).map_err(|e| format!("session: {}", e))?;
    let v = json::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("session: {}", e))?;
    let account = v.path("primaryAccounts").get(MAIL).str().to_string();
    if account.is_empty() || v.get("apiUrl").str().is_empty() {
        return Err("session: no mail account (does the token have mail access?)".into());
    }
    Ok(Session {
        api_url: v.get("apiUrl").str().into(),
        download_url: v.get("downloadUrl").str().into(),
        upload_url: v.get("uploadUrl").str().into(),
        event_url: v.get("eventSourceUrl").str().into(),
        username: v.get("username").str().into(),
        caps: match v.get("capabilities") {
            Value::Obj(m) => m.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        },
        account,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Mailboxes,
    Identities,
    /// A page of the message list: view generation, request sequence, and
    /// whether it replaces the list (refresh) or extends it.
    List(u64, u64, bool),
    /// An open conversation, by generation.
    Thread(u64),
    Act(&'static str),
    /// Sending or saving a draft; the id says which compose session.
    Send(&'static str),
    Upload(usize),
    Download(String),
    /// Bytes of an image for the gallery, by key.
    Image(String),
    /// A server state change, or (with no calls) a push (re)connection.
    Push,
    PushDown(String),
}

pub struct Resp {
    pub calls: Vec<(String, Value, String)>,
    pub bytes: Vec<u8>,
}

impl Resp {
    /// The result of the call with this id, or the server's error for it.
    pub fn get(&self, id: &str) -> Result<&Value, String> {
        match self.calls.iter().find(|c| c.2 == id) {
            Some((n, v, _)) if n == "error" => Err(format!("{}: {}", v.get("type").str(), v.get("description").str())),
            Some((_, v, _)) => Ok(v),
            None => Err(format!("no response for {}", id)),
        }
    }
}

pub type Reply = (Tag, Result<Resp, String>);

/// One method call: `["Email/get", {...}, "id"]`.
pub fn call(name: &str, args: &str, id: &str) -> String {
    format!("[{},{},{}]", quote(name), args, quote(id))
}

struct Auth {
    token: String,
    s: Session,
}

type Job = Box<dyn FnOnce(&mut Client) + Send>;

#[derive(Clone)]
pub struct Net {
    auth: Arc<Auth>,
    tx: Sender<Reply>,
    waker: Waker,
    jobs: Sender<Job>,
}

fn run(c: &mut Client, a: &Auth, calls: &[String]) -> Result<Resp, String> {
    let bearer = format!("Bearer {}", a.token);
    let using: Vec<String> = ["urn:ietf:params:jmap:core", MAIL, SUBMISSION].iter().filter(|c| a.s.caps.is_empty() || a.s.caps.iter().any(|x| x == *c)).map(|c| quote(c)).collect();
    let body = format!("{{\"using\":[{}],\"methodCalls\":[{}]}}", using.join(","), calls.join(","));
    let h = [("Authorization", bearer.as_str()), ("Accept", "application/json"), ("Content-Type", "application/json")];
    let res = c.request("POST", &a.s.api_url, &h, body.as_bytes())?;
    let text = String::from_utf8_lossy(&res.body);
    let v = json::parse(&text).map_err(|_| format!("HTTP {}", res.status))?;
    if res.status >= 400 || !v.get("type").is_null() {
        return Err(v.get("detail").opt_str().or(v.get("title").opt_str()).map(String::from).unwrap_or_else(|| format!("HTTP {}", res.status)));
    }
    let calls = v.get("methodResponses").arr().iter().map(|r| (r.arr().first().map_or("", |x| x.str()).to_string(), r.arr().get(1).cloned().unwrap_or(Value::Null), r.arr().get(2).map_or("", |x| x.str()).to_string())).collect();
    Ok(Resp { calls, bytes: Vec::new() })
}

fn fill(t: &str, s: &Session, extra: &[(&str, &str)]) -> String {
    let mut u = t.replace("{accountId}", &mhttp::urlencode(&s.account));
    for (k, v) in extra {
        u = u.replace(&format!("{{{}}}", k), &mhttp::urlencode(v));
    }
    u
}

impl Net {
    pub fn new(token: String, session: Session, tx: Sender<Reply>, waker: Waker) -> Net {
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
        Net { auth: Arc::new(Auth { token, s: session }), tx, waker, jobs }
    }

    pub fn account(&self) -> &str {
        &self.auth.s.account
    }

    fn submit(&self, f: impl FnOnce(&mut Client, &Auth) -> Reply + Send + 'static) {
        let n = self.clone();
        let _ = self.jobs.send(Box::new(move |c| {
            let r = f(c, &n.auth);
            let _ = n.tx.send(r);
            n.waker.wake();
        }));
    }

    /// Run method calls (built with `call`) as one request.
    pub fn call(&self, tag: Tag, calls: Vec<String>) {
        self.submit(move |c, a| (tag, run(c, a, &calls)));
    }

    pub fn download(&self, key: &str, blob: &str, name: &str, typ: &str) {
        let key = key.to_string();
        let (blob, name, typ) = (blob.to_string(), name.to_string(), typ.to_string());
        self.submit(move |c, a| {
            let url = fill(&a.s.download_url, &a.s, &[("blobId", &blob), ("name", &name), ("type", &typ)]);
            let bearer = format!("Bearer {}", a.token);
            let r = c.get_file(&url, &[("Authorization", &bearer)], &["Authorization"], 200 << 20);
            (Tag::Download(key), r.map(|bytes| Resp { calls: Vec::new(), bytes }))
        });
    }

    /// An image attachment, for the gallery (needs the account's credentials).
    pub fn fetch_blob(&self, key: &str, blob: &str, name: &str, typ: &str) {
        let key = key.to_string();
        let (blob, name, typ) = (blob.to_string(), name.to_string(), typ.to_string());
        self.submit(move |c, a| {
            let url = fill(&a.s.download_url, &a.s, &[("blobId", &blob), ("name", &name), ("type", &typ)]);
            let bearer = format!("Bearer {}", a.token);
            let r = c.get_file(&url, &[("Authorization", &bearer)], &["Authorization"], 12 << 20);
            (Tag::Image(key), r.map(|bytes| Resp { calls: Vec::new(), bytes }))
        });
    }

    /// A remote image from a message, fetched without any credentials.
    pub fn fetch_url(&self, key: &str, url: &str) {
        let (key, url) = (key.to_string(), url.to_string());
        self.submit(move |c, _| {
            let r = c.get_file(&url, &[("Accept", "image/png,image/jpeg,*/*")], &[], 12 << 20);
            (Tag::Image(key), r.map(|bytes| Resp { calls: Vec::new(), bytes }))
        });
    }

    /// Upload a file; the reply's first call is `("upload", {blobId, type, size}, "")`.
    pub fn upload(&self, i: usize, typ: &str, data: Vec<u8>) {
        let typ = typ.to_string();
        self.submit(move |c, a| {
            let url = fill(&a.s.upload_url, &a.s, &[]);
            let bearer = format!("Bearer {}", a.token);
            let r = c.request("POST", &url, &[("Authorization", &bearer), ("Content-Type", &typ)], &data).and_then(|r| {
                let v = json::parse(&String::from_utf8_lossy(&r.body)).map_err(|_| format!("upload failed (HTTP {})", r.status))?;
                if v.get("blobId").str().is_empty() {
                    return Err(format!("upload failed: {}", v.get("detail").str()));
                }
                Ok(Resp { calls: vec![("upload".into(), v, String::new())], bytes: Vec::new() })
            });
            (Tag::Upload(i), r)
        });
    }

    /// Keep an EventSource connection to the server open; every change to
    /// mail arrives as `Tag::Push`. Reconnects forever, and a reconnect is
    /// itself reported (as a Push with no calls) so the UI can resync.
    pub fn start_push(&self) {
        let (a, tx, waker) = (self.auth.clone(), self.tx.clone(), self.waker);
        if a.s.event_url.is_empty() {
            let _ = tx.send((Tag::PushDown("server offers no push".into()), Err(String::new())));
            waker.wake();
            return;
        }
        std::thread::spawn(move || {
            let url = fill(&a.s.event_url, &a.s, &[("types", "Email,Mailbox"), ("closeafter", "no"), ("ping", "30")]);
            let bearer = format!("Bearer {}", a.token);
            let mut wait = 2;
            let mut event = String::new();
            loop {
                let mut up = false;
                let r = mhttp::stream_lines(&url, &[("Authorization", &bearer)], Duration::from_secs(95), |line| {
                    if !up {
                        up = true;
                        let _ = tx.send((Tag::Push, Ok(Resp { calls: Vec::new(), bytes: Vec::new() })));
                        waker.wake();
                    }
                    if let Some(e) = line.strip_prefix("event:") {
                        event = e.trim().to_string();
                    } else if let Some(d) = line.strip_prefix("data:") {
                        if event == "state" || event.is_empty() {
                            if let Ok(v) = json::parse(d.trim()) {
                                let _ = tx.send((Tag::Push, Ok(Resp { calls: vec![("state".into(), v, String::new())], bytes: Vec::new() })));
                                waker.wake();
                            }
                        }
                    }
                    true
                });
                let why = match r {
                    Ok(()) => "closed".to_string(),
                    Err(e) => e,
                };
                let _ = tx.send((Tag::PushDown(why), Err(String::new())));
                waker.wake();
                wait = if up { 2 } else { (wait * 2).min(60) };
                std::thread::sleep(Duration::from_secs(wait));
            }
        });
    }
}

/// `Mailbox/get` with counts, and the identities we can send as.
pub fn mailboxes(account: &str) -> Vec<String> {
    vec![call("Mailbox/get", &format!("{{\"accountId\":{},\"properties\":[\"name\",\"parentId\",\"role\",\"sortOrder\",\"totalThreads\",\"unreadThreads\"]}}", quote(account)), "m")]
}

pub fn identities(account: &str) -> Vec<String> {
    vec![call("Identity/get", &format!("{{\"accountId\":{}}}", quote(account)), "i")]
}

const LIST_PROPS: &str = "\"threadId\",\"mailboxIds\",\"keywords\",\"from\",\"subject\",\"receivedAt\",\"preview\",\"hasAttachment\"";

/// One page of a list: query, the messages, and every message of their
/// conversations (so unread/star/archive know the whole thread).
pub fn list(account: &str, filter: &str, collapse: bool, position: usize, limit: usize) -> Vec<String> {
    let a = quote(account);
    vec![
        call("Email/query", &format!("{{\"accountId\":{},\"filter\":{},\"sort\":[{{\"property\":\"receivedAt\",\"isAscending\":false}}],\"collapseThreads\":{},\"position\":{},\"limit\":{},\"calculateTotal\":true}}", a, filter, collapse, position, limit), "q"),
        call("Email/get", &format!("{{\"accountId\":{},\"#ids\":{{\"resultOf\":\"q\",\"name\":\"Email/query\",\"path\":\"/ids\"}},\"properties\":[{}]}}", a, LIST_PROPS), "e"),
        call("Thread/get", &format!("{{\"accountId\":{},\"#ids\":{{\"resultOf\":\"e\",\"name\":\"Email/get\",\"path\":\"/list/*/threadId\"}}}}", a), "t"),
        call("Email/get", &format!("{{\"accountId\":{},\"#ids\":{{\"resultOf\":\"t\",\"name\":\"Thread/get\",\"path\":\"/list/*/emailIds\"}},\"properties\":[\"threadId\",\"mailboxIds\",\"keywords\",\"from\",\"receivedAt\"]}}", a), "all"),
    ]
}

/// A conversation with full bodies.
pub fn thread(account: &str, thread: &str) -> Vec<String> {
    let a = quote(account);
    vec![
        call("Thread/get", &format!("{{\"accountId\":{},\"ids\":[{}]}}", a, quote(thread)), "t"),
        call(
            "Email/get",
            &format!(
                "{{\"accountId\":{},\"#ids\":{{\"resultOf\":\"t\",\"name\":\"Thread/get\",\"path\":\"/list/*/emailIds\"}},\"properties\":[\"threadId\",\"mailboxIds\",\"keywords\",\"from\",\"to\",\"cc\",\"replyTo\",\"subject\",\"receivedAt\",\"sentAt\",\"messageId\",\"inReplyTo\",\"references\",\"attachments\",\"textBody\",\"bodyValues\"],\"bodyProperties\":[\"partId\",\"blobId\",\"type\",\"name\",\"size\",\"disposition\"],\"fetchTextBodyValues\":true,\"maxBodyValueBytes\":524288}}",
                a
            ),
            "e",
        ),
    ]
}

/// `Email/set` updates from (id, patch-json-object-body) pairs.
pub fn update(account: &str, patches: &[(String, String)]) -> Vec<String> {
    let body: Vec<String> = patches.iter().map(|(id, p)| format!("{}:{{{}}}", quote(id), p)).collect();
    vec![call("Email/set", &format!("{{\"accountId\":{},\"update\":{{{}}}}}", quote(account), body.join(",")), "u")]
}

pub fn destroy(account: &str, ids: &[String]) -> Vec<String> {
    let ids: Vec<String> = ids.iter().map(|i| quote(i)).collect();
    vec![call("Email/set", &format!("{{\"accountId\":{},\"destroy\":[{}]}}", quote(account), ids.join(",")), "u")]
}

/// Errors from an `Email/set` result: the first `notUpdated`/`notDestroyed`/`notCreated` entry.
pub fn set_error(v: &Value) -> Option<String> {
    for k in ["notUpdated", "notDestroyed", "notCreated"] {
        if let Value::Obj(m) = v.get(k) {
            if let Some((id, e)) = m.first() {
                let d = e.get("description").opt_str().unwrap_or(e.get("type").str());
                return Some(format!("{}: {}", id, d));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls() {
        assert_eq!(call("Foo/get", "{}", "x"), "[\"Foo/get\",{},\"x\"]");
        let l = list("A1", "{\"inMailbox\":\"i\"}", true, 0, 50);
        assert_eq!(l.len(), 4);
        assert!(l[0].contains("\"collapseThreads\":true") && l[1].contains("\"resultOf\":\"q\""));
        for c in &l {
            json::parse(c).unwrap();
        }
        for c in thread("A1", "T1").iter().chain(update("A1", &[("e1".into(), "\"keywords/$seen\":true".into())]).iter()) {
            json::parse(c).unwrap();
        }
    }

    #[test]
    fn errors() {
        let v = json::parse(r#"{"notUpdated":{"e1":{"type":"notFound","description":"gone"}}}"#).unwrap();
        assert_eq!(set_error(&v).as_deref(), Some("e1: gone"));
        let r = Resp { calls: vec![("error".into(), json::parse(r#"{"type":"unknownMethod"}"#).unwrap(), "a".into())], bytes: vec![] };
        assert!(r.get("a").is_err() && r.get("b").is_err());
    }

    #[test]
    fn template() {
        let s = Session { account: "u1".into(), ..Session::default() };
        assert_eq!(fill("https://h/{accountId}/{blobId}/{name}?type={type}", &s, &[("blobId", "B"), ("name", "a b.pdf"), ("type", "x/y")]), "https://h/u1/B/a%20b.pdf?type=x%2Fy");
    }
}
