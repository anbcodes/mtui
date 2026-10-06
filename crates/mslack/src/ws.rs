// Live events over a WebSocket (RFC 6455). Two Slack flavours share it:
// Socket Mode (app token; events come in envelopes that must be acked) and
// RTM (user or browser-session token; bare events, client sends pings).

use crate::http::{self, Client, Stream, Url};
use crate::net::{Reply, Tag};
use mtui::json::{self, Value};
use mtui::term::Waker;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// After this long without traffic we ping; two silent periods in a row
/// mean the link is dead.
const IDLE: Duration = Duration::from_secs(30);
const MAX_MSG: usize = 16 << 20;

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> (i % 16 * 8)) as u8 ^ i as u8;
        }
    }
    b
}

fn write_frame(s: &mut Stream, op: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut f = Vec::with_capacity(payload.len() + 14);
    f.push(0x80 | op);
    match payload.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n <= 0xffff => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mask: [u8; 4] = random();
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f)?;
    s.flush()
}

/// Read one complete message (reassembling fragments). Control frames are
/// returned as they arrive.
fn read_message(r: &mut BufReader<Stream>) -> Result<(u8, Vec<u8>), String> {
    let mut msg = Vec::new();
    let mut msg_op = 0;
    loop {
        let mut h = [0u8; 2];
        r.read_exact(&mut h).map_err(|e| e.to_string())?;
        let (fin, op, masked) = (h[0] & 0x80 != 0, h[0] & 0x0f, h[1] & 0x80 != 0);
        let len = match h[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        if len > MAX_MSG || msg.len() + len > MAX_MSG {
            return Err("message too large".into());
        }
        let mut mask = [0u8; 4];
        if masked {
            r.read_exact(&mut mask).map_err(|e| e.to_string())?;
        }
        let mut p = vec![0u8; len];
        r.read_exact(&mut p).map_err(|e| e.to_string())?;
        if masked {
            p.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
        }
        if op >= 8 {
            return Ok((op, p));
        }
        if op != 0 {
            msg_op = op;
        }
        msg.extend_from_slice(&p);
        if fin {
            return Ok((msg_op, msg));
        }
    }
}

pub enum Auth {
    /// Socket Mode with an app-level token (xapp-…).
    App(String),
    /// RTM with the user token (xoxp-… from classic apps, or xoxc-… plus
    /// the browser's `d` cookie).
    Rtm(String, Option<String>),
}

pub struct Socket {
    pub auth: Auth,
    pub base: String,
    pub tx: Sender<Reply>,
    pub waker: Waker,
}

/// Errors that retrying won't fix.
fn fatal(e: &str) -> bool {
    ["invalid_auth", "not_authed", "not_allowed_token_type", "missing_scope", "account_inactive", "token_revoked", "token_expired", "method_deprecated"].iter().any(|f| e.contains(f))
}

impl Socket {
    fn send(&self, tag: Tag, r: Result<Value, String>) {
        let _ = self.tx.send((tag, r));
        self.waker.wake();
    }

    /// Run on a background thread, reconnecting with backoff. Gives up
    /// (reporting "stopped: …") on errors that retrying won't fix.
    pub fn spawn(self) {
        std::thread::spawn(move || {
            let mut backoff = 1;
            loop {
                let r = self.session();
                if let Err(e) = &r {
                    if fatal(e) {
                        self.send(Tag::Live, Err(format!("stopped: {}", e)));
                        return;
                    }
                }
                let quick = matches!(&r, Ok(reason) if ["refresh_requested", "warning", "goodbye"].contains(&reason.as_str()));
                self.send(Tag::Live, Err(r.unwrap_or_else(|e| e)));
                if quick {
                    backoff = 1;
                    continue;
                }
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(60);
            }
        });
    }

    /// Ask Slack for a WebSocket URL.
    fn open_url(&self) -> Result<String, String> {
        let (method, token, cookie) = match &self.auth {
            Auth::App(t) => ("apps.connections.open", t, None),
            Auth::Rtm(t, c) => ("rtm.connect", t, c.as_ref()),
        };
        let bearer = format!("Bearer {}", token);
        let cookie = cookie.map(|c| format!("d={}", c));
        let mut h = vec![("Authorization", bearer.as_str()), ("Content-Type", "application/x-www-form-urlencoded")];
        if let Some(c) = &cookie {
            h.push(("Cookie", c));
        }
        let res = Client::new().request("POST", &format!("{}/{}", self.base, method), &h, b"")?;
        let v = json::parse(&String::from_utf8_lossy(&res.body)).map_err(|e| format!("HTTP {}: {}", res.status, e))?;
        if !v.get("ok").bool() {
            return Err(format!("{}: {}", method, v.get("error").str()));
        }
        Ok(v.get("url").str().to_string())
    }

    /// One connection's lifetime. Ok(reason) for an orderly disconnect.
    fn session(&self) -> Result<String, String> {
        let u = Url::parse(&self.open_url()?)?;
        let rtm = matches!(self.auth, Auth::Rtm(..));
        let mut s = http::connect(&u, IDLE)?;
        let key = mtui::base64::encode(&random::<16>());
        let mut req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n", u.path, u.host, key);
        if let Auth::Rtm(_, Some(c)) = &self.auth {
            req += &format!("Cookie: d={}\r\n", c);
        }
        req += "\r\n";
        s.write_all(req.as_bytes()).and_then(|_| s.flush()).map_err(|e| e.to_string())?;
        let mut r = BufReader::new(s);
        let (status, _) = http::read_head(&mut r).map_err(|e| e.to_string())?;
        // TLS already authenticates the server, so Sec-WebSocket-Accept
        // isn't checked.
        if status != 101 {
            return Err(format!("websocket upgrade failed: HTTP {}", status));
        }
        let (mut silent, mut ping_id) = (0, 0u64);
        loop {
            // Wait for the next frame without consuming anything, so an idle
            // timeout can't split a frame.
            match r.fill_buf() {
                Ok([]) => return Ok("closed by server".into()),
                Ok(_) => silent = 0,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    silent += 1;
                    if silent > 2 {
                        return Err("timed out".into());
                    }
                    ping_id += 1;
                    let res = if rtm { write_frame(r.get_mut(), 1, format!("{{\"id\":{},\"type\":\"ping\"}}", ping_id).as_bytes()) } else { write_frame(r.get_mut(), 9, b"") };
                    res.map_err(|e| e.to_string())?;
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            }
            let (op, p) = read_message(&mut r)?;
            match op {
                1 => {
                    let Ok(m) = json::parse(&String::from_utf8_lossy(&p)) else { continue };
                    if let Some(id) = m.get("envelope_id").opt_str() {
                        let ack = format!("{{\"envelope_id\":{}}}", json::quote(id));
                        write_frame(r.get_mut(), 1, ack.as_bytes()).map_err(|e| e.to_string())?;
                    }
                    match m.get("type").str() {
                        "hello" => self.send(Tag::Live, Ok(Value::Null)),
                        "disconnect" => return Ok(m.get("reason").str().to_string()),
                        "goodbye" => return Ok("goodbye".into()),
                        "events_api" => self.send(Tag::Event, Ok(m.path("payload.event").clone())),
                        "pong" | "reconnect_url" | "" => {}
                        _ if rtm && m.get("reply_to").is_null() => self.send(Tag::Event, Ok(m)),
                        _ => {}
                    }
                }
                8 => return Ok("closed by server".into()),
                9 => write_frame(r.get_mut(), 10, &p).map_err(|e| e.to_string())?,
                _ => {}
            }
        }
    }
}
