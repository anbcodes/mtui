// Writing mail: a draft is edited as text (headers, a blank line, the body) in
// $EDITOR, parsed back, and turned into JMAP. Replies and forwards fill it in.

use crate::jmap::{self, call};
use crate::model::{join_addrs, parse_addrs, Addr, Att, Identity, Msg};
use crate::timefmt;
use mtui::json::quote;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Local files to attach.
    pub attach: Vec<String>,
    /// Attachments already on the server (forwarded, or on a draft being edited).
    pub keep: Vec<Att>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    /// A saved draft this one supersedes.
    pub replaces: Option<String>,
    /// Message to mark answered / forwarded once sent.
    pub answered: Option<String>,
    pub forwarded: Option<String>,
}

impl Draft {
    /// The text edited in $EDITOR, and the (1-based) line the body starts on.
    pub fn to_text(&self) -> (String, usize) {
        let mut s = format!("From: {}\nTo: {}\nCc: {}\nBcc: {}\nSubject: {}\n", self.from, self.to, self.cc, self.bcc, self.subject);
        for a in &self.keep {
            s += &format!("Keep: {}\n", a.name);
        }
        if self.attach.is_empty() {
            s += "Attach: \n";
        }
        for a in &self.attach {
            s += &format!("Attach: {}\n", a);
        }
        s.push('\n');
        let line = s.lines().count() + 1;
        s += &self.body;
        (s, line)
    }

    /// Read the edited text back; envelope facts (reply ids, kept
    /// attachments) come from `self`, minus any `Keep:` line the user deleted.
    pub fn parse(&self, text: &str) -> Draft {
        let mut d = Draft { in_reply_to: self.in_reply_to.clone(), references: self.references.clone(), replaces: self.replaces.clone(), answered: self.answered.clone(), forwarded: self.forwarded.clone(), ..Draft::default() };
        let text = text.replace("\r\n", "\n");
        let (head, body) = match text.split_once("\n\n") {
            Some((h, b)) => (h.to_string(), b.to_string()),
            None => (text.trim_end_matches('\n').to_string(), String::new()),
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        for l in head.lines() {
            if l.starts_with([' ', '\t']) {
                if let Some(last) = fields.last_mut() {
                    last.1 += " ";
                    last.1 += l.trim();
                }
            } else if let Some((k, v)) = l.split_once(':') {
                fields.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        let mut keep = Vec::new();
        for (k, v) in fields {
            let join = |a: &mut String, v: &str| {
                if !v.is_empty() {
                    if !a.is_empty() {
                        a.push_str(", ");
                    }
                    a.push_str(v);
                }
            };
            match k.as_str() {
                "from" => d.from = v,
                "to" => join(&mut d.to, &v),
                "cc" => join(&mut d.cc, &v),
                "bcc" => join(&mut d.bcc, &v),
                "subject" => d.subject = v,
                "attach" if !v.is_empty() => d.attach.push(v),
                "keep" => keep.push(v),
                _ => {}
            }
        }
        d.keep = self.keep.iter().filter(|a| keep.contains(&a.name)).cloned().collect();
        d.body = body;
        d
    }

    pub fn is_blank(&self) -> bool {
        self.to.trim().is_empty() && self.cc.trim().is_empty() && self.subject.trim().is_empty() && self.body.trim().is_empty() && self.attach.is_empty() && self.keep.is_empty()
    }

    /// Check it can be sent; returns the identity to send as.
    pub fn check<'a>(&self, ids: &'a [Identity]) -> Result<&'a Identity, String> {
        if parse_addrs(&format!("{},{},{}", self.to, self.cc, self.bcc)).is_empty() {
            return Err("no recipients (To: needs an address)".into());
        }
        let from = parse_addrs(&self.from);
        let from = from.first().ok_or("From: needs an address")?;
        ids.iter().find(|i| i.matches(&from.email)).ok_or_else(|| format!("you can't send as {} (identities: {})", from.email, ids.iter().map(|i| i.email.as_str()).collect::<Vec<_>>().join(", ")))
    }

    fn addrs_json(s: &str) -> String {
        let a: Vec<String> = parse_addrs(s).iter().map(|a| format!("{{\"name\":{},\"email\":{}}}", if a.name.is_empty() { "null".to_string() } else { quote(&a.name) }, quote(&a.email))).collect();
        format!("[{}]", a.join(","))
    }

    /// The `Email/set` create object. `files` are the uploaded attachments;
    /// `keep` ones are included by reference.
    pub fn create_json(&self, drafts: &str, files: &[Att]) -> String {
        let mut o = format!("{{\"mailboxIds\":{{{}:true}},\"keywords\":{{\"$draft\":true,\"$seen\":true}},\"from\":{},\"to\":{},\"subject\":{}", quote(drafts), Self::addrs_json(&self.from), Self::addrs_json(&self.to), quote(&self.subject));
        for (k, v) in [("cc", &self.cc), ("bcc", &self.bcc)] {
            if !parse_addrs(v).is_empty() {
                o += &format!(",\"{}\":{}", k, Self::addrs_json(v));
            }
        }
        let ids = |v: &[String]| format!("[{}]", v.iter().map(|s| quote(s)).collect::<Vec<_>>().join(","));
        if !self.in_reply_to.is_empty() {
            o += &format!(",\"inReplyTo\":{}", ids(&self.in_reply_to));
        }
        if !self.references.is_empty() {
            o += &format!(",\"references\":{}", ids(&self.references));
        }
        let text = "{\"partId\":\"b\",\"type\":\"text/plain\"}";
        let all: Vec<&Att> = self.keep.iter().chain(files.iter()).collect();
        if all.is_empty() {
            o += &format!(",\"bodyStructure\":{}", text);
        } else {
            let subs: Vec<String> = all.iter().map(|a| format!("{{\"blobId\":{},\"type\":{},\"name\":{},\"disposition\":\"attachment\"}}", quote(&a.blob), quote(&a.typ), quote(&a.name))).collect();
            o += &format!(",\"bodyStructure\":{{\"type\":\"multipart/mixed\",\"subParts\":[{},{}]}}", text, subs.join(","));
        }
        o += &format!(",\"bodyValues\":{{\"b\":{{\"value\":{}}}}}}}", quote(&self.body));
        o
    }

    /// Calls that save this draft in the Drafts folder, replacing `replaces`.
    pub fn save_calls(&self, account: &str, drafts: &str, files: &[Att]) -> Vec<String> {
        let destroy = self.replaces.as_ref().map_or(String::new(), |r| format!(",\"destroy\":[{}]", quote(r)));
        vec![call("Email/set", &format!("{{\"accountId\":{},\"create\":{{\"c\":{}}}{}}}", quote(account), self.create_json(drafts, files), destroy), "d")]
    }

    /// Calls that save the message and submit it; once the server accepts
    /// it, it leaves Drafts for Sent, like in the web client.
    pub fn send_calls(&self, account: &str, drafts: &str, sent: Option<&str>, identity: &str, files: &[Att]) -> Vec<String> {
        let mut c = self.save_calls(account, drafts, files);
        let mut after = format!("\"keywords/$draft\":null,\"mailboxIds/{}\":null", drafts);
        if let Some(s) = sent {
            after += &format!(",\"mailboxIds/{}\":true", s);
        }
        c.push(call("EmailSubmission/set", &format!("{{\"accountId\":{},\"create\":{{\"s\":{{\"identityId\":{},\"emailId\":\"#c\"}}}},\"onSuccessUpdateEmail\":{{\"#s\":{{{}}}}}}}", quote(account), quote(identity), after), "s"));
        for (id, kw) in [(&self.answered, "$answered"), (&self.forwarded, "$forwarded")] {
            if let Some(id) = id {
                c.extend(jmap::update(account, &[(id.clone(), format!("\"keywords/{}\":true", kw))]));
            }
        }
        c
    }
}

fn re_subject(s: &str, prefix: &str) -> String {
    let has = s.trim_start().to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase());
    if has {
        s.trim().to_string()
    } else {
        format!("{} {}", prefix, s.trim())
    }
}

fn quoted(text: &str) -> String {
    text.lines().map(|l| if l.starts_with('>') { format!(">{}", l) } else if l.is_empty() { ">".into() } else { format!("> {}", l) }).collect::<Vec<_>>().join("\n")
}

fn signature(id: &Identity) -> String {
    if id.signature.trim().is_empty() {
        String::new()
    } else {
        format!("\n-- \n{}\n", id.signature.trim_end())
    }
}

/// Who to answer as: the identity a message was addressed to (as the exact
/// address it used, for wildcard identities), else the default.
fn identity_for<'a>(m: &Msg, ids: &'a [Identity]) -> Option<(&'a Identity, Addr)> {
    let hit = m.to.iter().chain(m.cc.iter()).find_map(|a| ids.iter().find(|i| i.matches(&a.email)).map(|i| (i, a)));
    match hit {
        Some((i, a)) if i.email.starts_with('*') => Some((i, Addr { name: i.name.clone(), email: a.email.clone() })),
        Some((i, _)) => Some((i, i.addr())),
        None => default_identity(ids).map(|i| (i, i.addr())),
    }
}

fn default_identity(ids: &[Identity]) -> Option<&Identity> {
    ids.iter().find(|i| !i.email.starts_with('*')).or(ids.first())
}

pub fn new_mail(ids: &[Identity], to: &str) -> Draft {
    let id = default_identity(ids);
    Draft { from: id.map_or(String::new(), |i| i.addr().full()), to: to.into(), body: id.map_or(String::new(), |i| format!("\n{}", signature(i))), ..Draft::default() }
}

pub fn reply(m: &Msg, all: bool, ids: &[Identity]) -> Draft {
    let (id, from) = identity_for(m, ids).map_or((None, Addr::default()), |(i, a)| (Some(i), a));
    let mine = |a: &Addr| ids.iter().any(|i| i.matches(&a.email));
    let sender = if m.reply_to.is_empty() { &m.from } else { &m.reply_to };
    // Answering a message of my own continues the conversation with its recipients.
    let (mut to, mut cc): (Vec<Addr>, Vec<Addr>) = if m.from.iter().any(&mine) { (m.to.clone(), m.cc.clone()) } else { (sender.clone(), if all { m.to.iter().chain(m.cc.iter()).filter(|a| !mine(a)).cloned().collect() } else { Vec::new() }) };
    if !all {
        cc.clear();
    }
    to.dedup_by(|a, b| a.email.eq_ignore_ascii_case(&b.email));
    cc.retain(|a| !to.iter().any(|t| t.email.eq_ignore_ascii_case(&a.email)));
    let who = m.from.first().map_or("someone".to_string(), |a| a.full());
    let mut refs = m.references.clone();
    refs.extend(m.message_id.iter().cloned());
    Draft {
        from: from.full(),
        to: join_addrs(&to),
        cc: join_addrs(&cc),
        subject: re_subject(&m.subject, "Re:"),
        body: format!("\n{}\nOn {}, {} wrote:\n{}\n", id.map_or(String::new(), signature), timefmt::long(&m.sent), who, quoted(m.text.trim_end())),
        in_reply_to: m.message_id.clone(),
        references: refs,
        answered: Some(m.mini.id.clone()),
        ..Draft::default()
    }
}

pub fn forward(m: &Msg, ids: &[Identity]) -> Draft {
    let (id, from) = identity_for(m, ids).map_or((None, Addr::default()), |(i, a)| (Some(i), a));
    let mut body = format!("\n{}\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n", id.map_or(String::new(), signature), join_addrs(&m.from), timefmt::long(&m.sent), m.subject, join_addrs(&m.to));
    if !m.cc.is_empty() {
        body += &format!("Cc: {}\n", join_addrs(&m.cc));
    }
    body += &format!("\n{}\n", m.text.trim_end());
    Draft { from: from.full(), subject: re_subject(&m.subject, "Fwd:"), body, keep: m.atts.clone(), forwarded: Some(m.mini.id.clone()), ..Draft::default() }
}

/// A saved draft, reopened for editing.
pub fn from_draft(m: &Msg) -> Draft {
    Draft { from: join_addrs(&m.from), to: join_addrs(&m.to), cc: join_addrs(&m.cc), subject: m.subject.clone(), body: m.text.clone(), keep: m.atts.clone(), in_reply_to: m.in_reply_to.clone(), references: m.references.clone(), replaces: Some(m.mini.id.clone()), ..Draft::default() }
}

pub fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("txt" | "md" | "log") => "text/plain",
        Some("html" | "htm") => "text/html",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

pub fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{}/{}", h, rest),
        _ => p.to_string(),
    }
}

/// Write `text` to a new private temp file for the editor.
pub fn write_temp(text: &str) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!("mmail-{}-{}.eml", std::process::id(), timefmt::now()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Run the user's `$VISUAL` / `$EDITOR` on `text` and return the edited text.
/// The caller has already left raw mode.
pub fn edit(text: &str, body_line: usize) -> Result<String, String> {
    let path = write_temp(text)?;
    let spec = ["VISUAL", "EDITOR"].iter().find_map(|k| std::env::var(k).ok().filter(|s| !s.trim().is_empty())).unwrap_or_else(|| "vi".into());
    let mut words = spec.split_whitespace();
    let prog = words.next().unwrap_or("vi");
    let mut cmd = std::process::Command::new(prog);
    cmd.args(words);
    let base = prog.rsplit('/').next().unwrap_or(prog);
    if matches!(base, "vi" | "vim" | "nvim" | "nano" | "mvi") {
        cmd.arg(format!("+{}", body_line));
    }
    let status = cmd.arg(&path).status();
    let out = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    match status {
        Ok(s) if s.success() => out.map_err(|e| e.to_string()),
        Ok(s) => Err(format!("{} exited with {}", prog, s)),
        Err(e) => Err(format!("{}: {}", prog, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtui::json;

    fn ids() -> Vec<Identity> {
        vec![Identity { id: "i1".into(), name: "Me".into(), email: "me@fm.com".into(), signature: "Cheers".into() }, Identity { id: "i2".into(), name: "Alias".into(), email: "*@alias.org".into(), signature: String::new() }]
    }

    fn msg() -> Msg {
        Msg {
            from: vec![Addr { name: "Al".into(), email: "al@x.com".into() }],
            to: vec![Addr { name: String::new(), email: "me@fm.com".into() }, Addr { name: "Bo".into(), email: "bo@x.com".into() }],
            cc: vec![Addr { name: String::new(), email: "x@alias.org".into() }],
            subject: "Lunch?".into(),
            sent: "2026-10-05T12:00:00Z".into(),
            message_id: vec!["m2@x".into()],
            references: vec!["m1@x".into()],
            text: "Fancy lunch?\n> earlier".into(),
            mini: crate::model::Mini { id: "e9".into(), ..Default::default() },
            ..Msg::default()
        }
    }

    #[test]
    fn replies() {
        let r = reply(&msg(), false, &ids());
        assert_eq!((r.to.as_str(), r.cc.as_str(), r.subject.as_str()), ("Al <al@x.com>", "", "Re: Lunch?"));
        assert!(r.body.contains("wrote:\n> Fancy lunch?\n>> earlier"));
        assert_eq!(r.references, vec!["m1@x", "m2@x"]);
        assert_eq!(r.from, "Me <me@fm.com>");
        let all = reply(&msg(), true, &ids());
        assert_eq!(all.to, "Al <al@x.com>");
        assert_eq!(all.cc, "Bo <bo@x.com>");
        let mut m = msg();
        m.subject = "RE: Lunch?".into();
        m.to = vec![Addr { name: String::new(), email: "q@alias.org".into() }];
        let r = reply(&m, false, &ids());
        assert_eq!((r.subject.as_str(), r.from.as_str()), ("RE: Lunch?", "Alias <q@alias.org>"));
        let f = forward(&msg(), &ids());
        assert!(f.subject == "Fwd: Lunch?" && f.body.contains("Forwarded message") && f.forwarded.as_deref() == Some("e9"));
    }

    #[test]
    fn edit_round_trip() {
        let mut d = reply(&msg(), true, &ids());
        d.keep = vec![Att { blob: "B1".into(), name: "a.pdf".into(), typ: "application/pdf".into(), size: 1 }, Att { blob: "B2".into(), name: "b.png".into(), typ: "image/png".into(), size: 1 }];
        let (text, line) = d.to_text();
        assert_eq!(text.lines().nth(line - 2), Some(""));
        let edited = text.replace("Keep: b.png\n", "").replace("Attach: \n", "Attach: ~/notes.txt\n").replace("To: Al <al@x.com>", "To: Al <al@x.com>,\n  new@y.org");
        let p = d.parse(&edited);
        assert_eq!(p.to, "Al <al@x.com>, new@y.org");
        assert_eq!(p.keep.len(), 1);
        assert_eq!(p.attach, vec!["~/notes.txt"]);
        assert_eq!(p.in_reply_to, d.in_reply_to);
        assert_eq!(p.body, d.body);
        assert!(p.check(&ids()).is_ok());
        let bad = Draft { from: "me@elsewhere.com".into(), ..p.clone() };
        assert!(bad.check(&ids()).is_err());
        assert!(Draft { to: String::new(), cc: String::new(), ..p }.check(&ids()).is_err());
    }

    #[test]
    fn json_out() {
        let mut d = reply(&msg(), false, &ids());
        d.body = "Hi \"there\"\n".into();
        let files = vec![Att { blob: "B9".into(), name: "n.txt".into(), typ: "text/plain".into(), size: 3 }];
        let c = d.send_calls("A1", "dr", Some("snt"), "i1", &files);
        assert_eq!(c.len(), 3);
        for x in &c {
            json::parse(x).unwrap();
        }
        assert!(c[0].contains("\"multipart/mixed\"") && c[0].contains("\"blobId\":\"B9\"") && c[0].contains("\\\"there\\\""));
        assert!(c[1].contains("\"mailboxIds/snt\":true") && c[1].contains("\"mailboxIds/dr\":null") && c[1].contains("\"#c\""));
        assert!(c[2].contains("\"keywords/$answered\":true"));
        let plain = Draft { replaces: Some("old".into()), ..d }.save_calls("A1", "dr", &[]);
        assert!(plain[0].contains("\"destroy\":[\"old\"]") && plain[0].contains("\"type\":\"text/plain\""));
    }
}
