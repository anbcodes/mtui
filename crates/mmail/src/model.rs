// What the server tells us, parsed out of JMAP values.

use crate::html;
use mtui::json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub role: Option<String>,
    pub sort: i64,
    pub unread: i64,
    pub total: i64,
    /// Nesting depth in the folder tree (set by `tree`).
    pub depth: usize,
}

impl Mailbox {
    pub fn parse(v: &Value) -> Mailbox {
        Mailbox {
            id: v.get("id").str().into(),
            name: v.get("name").str().into(),
            parent: v.get("parentId").opt_str().map(String::from),
            role: v.get("role").opt_str().map(String::from),
            sort: v.get("sortOrder").num() as i64,
            unread: v.get("unreadThreads").num() as i64,
            total: v.get("totalThreads").num() as i64,
            depth: 0,
        }
    }
}

/// Mailboxes in display order: siblings by sortOrder then name, children under parents.
pub fn tree(mut all: Vec<Mailbox>) -> Vec<Mailbox> {
    all.sort_by_key(|m| (m.sort, m.name.to_lowercase()));
    let ids: Vec<String> = all.iter().map(|m| m.id.clone()).collect();
    let mut out = Vec::with_capacity(all.len());
    fn walk(all: &[Mailbox], ids: &[String], parent: Option<&str>, depth: usize, out: &mut Vec<Mailbox>) {
        for m in all {
            let top = m.parent.as_ref().is_none_or(|p| !ids.contains(p));
            let here = match parent {
                None => top,
                Some(p) => m.parent.as_deref() == Some(p),
            };
            if here && depth < 8 {
                out.push(Mailbox { depth, ..m.clone() });
                walk(all, ids, Some(&m.id), depth + 1, out);
            }
        }
    }
    walk(&all, &ids, None, 0, &mut out);
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

impl Addr {
    pub fn parse_list(v: &Value) -> Vec<Addr> {
        v.arr().iter().map(|a| Addr { name: a.get("name").str().trim().into(), email: a.get("email").str().trim().into() }).filter(|a| !a.email.is_empty()).collect()
    }

    /// The name if there is one, else the address.
    pub fn short(&self) -> &str {
        if self.name.is_empty() {
            &self.email
        } else {
            &self.name
        }
    }

    /// `Name <addr>` as it goes in a header.
    pub fn full(&self) -> String {
        if self.name.is_empty() {
            self.email.clone()
        } else if self.name.chars().any(|c| matches!(c, ',' | '<' | '>' | '"' | '@' | ';' | ':' | '(' | ')')) {
            format!("\"{}\" <{}>", self.name.replace('\\', "\\\\").replace('"', "\\\""), self.email)
        } else {
            format!("{} <{}>", self.name, self.email)
        }
    }
}

pub fn join_addrs(a: &[Addr]) -> String {
    a.iter().map(|x| x.full()).collect::<Vec<_>>().join(", ")
}

/// Split a header-style address list: `Bob <b@x>, "Smith, Al" <a@y>, c@z`.
pub fn parse_addrs(s: &str) -> Vec<Addr> {
    let mut parts = Vec::new();
    let (mut cur, mut inq, mut ang) = (String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => {
                inq = !inq;
                cur.push(c);
            }
            '<' if !inq => {
                ang = true;
                cur.push(c);
            }
            '>' if !inq => {
                ang = false;
                cur.push(c);
            }
            ',' | ';' if !inq && !ang => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    parts
        .iter()
        .filter_map(|p| {
            let p = p.trim();
            if p.is_empty() {
                return None;
            }
            Some(match (p.rfind('<'), p.rfind('>')) {
                (Some(a), Some(b)) if a < b => Addr { name: p[..a].trim().trim_matches('"').replace("\\\"", "\"").trim().into(), email: p[a + 1..b].trim().into() },
                _ => Addr { name: String::new(), email: p.into() },
            })
        })
        .filter(|a| a.email.contains('@'))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mini {
    pub id: String,
    pub boxes: Vec<String>,
    pub keywords: Vec<String>,
    pub from: Vec<Addr>,
    pub date: String,
}

impl Mini {
    pub fn parse(v: &Value) -> Mini {
        Mini { id: v.get("id").str().into(), boxes: keys(v.get("mailboxIds")), keywords: keys(v.get("keywords")), from: Addr::parse_list(v.get("from")), date: v.get("receivedAt").str().into() }
    }

    pub fn has(&self, k: &str) -> bool {
        self.keywords.iter().any(|x| x == k)
    }

    pub fn unread(&self) -> bool {
        !self.has("$seen") && !self.has("$draft")
    }
}

fn keys(v: &Value) -> Vec<String> {
    match v {
        Value::Obj(m) => m.iter().filter(|(_, v)| v.bool()).map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    }
}

/// One conversation in a list.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub thread: String,
    /// The message the list shows (the newest match).
    pub id: String,
    pub subject: String,
    pub preview: String,
    pub date: String,
    pub attach: bool,
    pub draft: bool,
    pub emails: Vec<Mini>,
    pub unread: bool,
    pub flagged: bool,
    /// Who wrote in the conversation, oldest first.
    pub people: Vec<String>,
    pub label: String,
}

impl Row {
    /// `threads` maps thread id to its messages.
    pub fn parse(v: &Value, threads: &HashMap<String, Vec<Mini>>, view: Option<&str>) -> Row {
        let thread: String = v.get("threadId").str().into();
        let me = Mini::parse(v);
        let emails = threads.get(&thread).cloned().filter(|e| !e.is_empty()).unwrap_or_else(|| vec![me.clone()]);
        let mut r = Row {
            thread,
            id: me.id.clone(),
            subject: v.get("subject").str().trim().into(),
            preview: v.get("preview").str().replace(['\n', '\r', '\t'], " ").trim().into(),
            date: me.date.clone(),
            attach: v.get("hasAttachment").bool(),
            draft: me.has("$draft"),
            emails,
            ..Row::default()
        };
        r.recompute(view);
        r
    }

    /// Derived fields; call after the messages change. Only messages in the
    /// mailbox being viewed count toward unread and star.
    pub fn recompute(&mut self, view: Option<&str>) {
        let here = |m: &&Mini| view.is_none_or(|b| m.boxes.iter().any(|x| x == b));
        self.unread = self.emails.iter().filter(here).any(|m| m.unread());
        self.flagged = self.emails.iter().filter(here).any(|m| m.has("$flagged"));
        let mut people: Vec<String> = Vec::new();
        for m in self.emails.iter().filter(here) {
            if let Some(a) = m.from.first() {
                let n = a.short().to_string();
                people.retain(|p| *p != n);
                people.push(n);
            }
        }
        self.people = people;
        self.label = format!("{} {} {}", self.people.join(" "), self.subject, self.preview);
    }

    /// Messages of this conversation that sit in mailbox `b`.
    pub fn in_box(&self, b: &str) -> Vec<&Mini> {
        self.emails.iter().filter(|m| m.boxes.iter().any(|x| x == b)).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Att {
    pub blob: String,
    pub name: String,
    pub typ: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub mini: Mini,
    pub from: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub sent: String,
    pub message_id: Vec<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub text: String,
    pub links: Vec<String>,
    pub atts: Vec<Att>,
    /// Images in the HTML that live on other servers (not loaded unless asked).
    pub remote: Vec<String>,
    pub expanded: bool,
}

/// An image to show under a message.
#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    /// The gallery key: `blob:ID` or the URL.
    pub key: String,
    pub name: String,
    pub att: Option<Att>,
}

impl Msg {
    /// Image attachments, and (if `remote`) the images the HTML links to.
    pub fn pics(&self, remote: bool) -> Vec<Pic> {
        let mut out: Vec<Pic> = self
            .atts
            .iter()
            .filter(|a| matches!(a.typ.to_ascii_lowercase().as_str(), "image/png" | "image/jpeg" | "image/jpg"))
            .map(|a| Pic { key: format!("blob:{}", a.blob), name: a.name.clone(), att: Some(a.clone()) })
            .collect();
        if remote {
            out.extend(self.remote.iter().take(8).map(|u| Pic { key: u.clone(), name: "remote image".into(), att: None }));
        }
        out
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.arr().iter().map(|s| s.str().to_string()).filter(|s| !s.is_empty()).collect()
}

impl Msg {
    pub fn parse(v: &Value) -> Msg {
        let mini = Mini::parse(v);
        let values = v.get("bodyValues");
        let mut parts: Vec<String> = Vec::new();
        let mut links = Vec::new();
        let mut remote = Vec::new();
        for p in v.get("textBody").arr() {
            let t = values.get(p.get("partId").str()).get("value").str();
            if p.get("type").str().eq_ignore_ascii_case("text/html") {
                let (text, l) = html::to_text(t);
                remote.extend(html::remote_images(t));
                parts.push(text);
                links.extend(l);
            } else {
                parts.push(t.replace("\r\n", "\n").replace('\r', "\n"));
                links.extend(html::find_urls(t));
            }
        }
        links.dedup();
        let atts = v
            .get("attachments")
            .arr()
            .iter()
            .map(|a| Att { blob: a.get("blobId").str().into(), name: a.get("name").opt_str().map(String::from).unwrap_or_else(|| "attachment".into()), typ: a.get("type").str().into(), size: a.get("size").num() as u64 })
            .collect();
        Msg {
            from: Addr::parse_list(v.get("from")),
            to: Addr::parse_list(v.get("to")),
            cc: Addr::parse_list(v.get("cc")),
            reply_to: Addr::parse_list(v.get("replyTo")),
            subject: v.get("subject").str().trim().into(),
            sent: v.get("sentAt").opt_str().unwrap_or(&mini.date).into(),
            message_id: strings(v.get("messageId")),
            in_reply_to: strings(v.get("inReplyTo")),
            references: strings(v.get("references")),
            text: parts.join("\n\n").replace('\t', "    "),
            links,
            atts,
            remote,
            expanded: false,
            mini,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub email: String,
    pub signature: String,
}

impl Identity {
    pub fn parse(v: &Value) -> Identity {
        Identity { id: v.get("id").str().into(), name: v.get("name").str().into(), email: v.get("email").str().into(), signature: v.get("textSignature").str().into() }
    }

    /// Does this identity send as `addr` (exact, or a `*@domain` wildcard)?
    pub fn matches(&self, addr: &str) -> bool {
        let (a, e) = (addr.to_ascii_lowercase(), self.email.to_ascii_lowercase());
        match e.strip_prefix('*') {
            Some(dom) => a.ends_with(dom),
            None => a == e,
        }
    }

    pub fn addr(&self) -> Addr {
        Addr { name: self.name.clone(), email: self.email.clone() }
    }
}

pub fn human_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{} B", n),
        1024..=1048575 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1048576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtui::json;

    #[test]
    fn addresses() {
        let a = parse_addrs("Bob <b@x.com>, \"Smith, Al\" <a@y.org>; c@z.net, junk");
        assert_eq!(a.len(), 3);
        assert_eq!((a[0].name.as_str(), a[0].email.as_str()), ("Bob", "b@x.com"));
        assert_eq!(a[1].name, "Smith, Al");
        assert_eq!(a[1].full(), "\"Smith, Al\" <a@y.org>");
        assert_eq!(a[2].email, "c@z.net");
    }

    #[test]
    fn folder_tree() {
        let m = |id: &str, parent: Option<&str>, sort| Mailbox { id: id.into(), name: id.into(), parent: parent.map(String::from), sort, ..Mailbox::default() };
        let t = tree(vec![m("b", None, 2), m("a1", Some("a"), 1), m("a", None, 1), m("orphan", Some("gone"), 3)]);
        let order: Vec<(&str, usize)> = t.iter().map(|m| (m.id.as_str(), m.depth)).collect();
        assert_eq!(order, vec![("a", 0), ("a1", 1), ("b", 0), ("orphan", 0)]);
    }

    #[test]
    fn rows_and_messages() {
        let v = json::parse(r#"{"id":"e2","threadId":"t1","mailboxIds":{"inbox":true},"keywords":{"$seen":true},"from":[{"name":"Al","email":"al@x"}],"subject":" Hi ","preview":"yo","receivedAt":"2026-10-06T11:00:00Z","hasAttachment":false}"#).unwrap();
        let mut th = HashMap::new();
        th.insert("t1".to_string(), vec![Mini { id: "e1".into(), boxes: vec!["inbox".into()], keywords: vec![], from: vec![Addr { name: "Bo".into(), email: "bo@x".into() }], date: String::new() }, Mini::parse(&v)]);
        let r = Row::parse(&v, &th, Some("inbox"));
        assert!(r.unread && !r.flagged);
        assert_eq!(r.people, vec!["Bo", "Al"]);
        assert_eq!(r.subject, "Hi");
        let mut r2 = r.clone();
        r2.emails[0].keywords.push("$seen".into());
        r2.recompute(Some("inbox"));
        assert!(!r2.unread);
        let m = json::parse(r#"{"id":"e","from":[{"email":"a@b"}],"textBody":[{"partId":"1","type":"text/html"}],"bodyValues":{"1":{"value":"<p>Hi <a href=\"https://q.io\">there</a></p>"}},"attachments":[{"blobId":"B","name":"x.pdf","type":"application/pdf","size":2048}]}"#).unwrap();
        let m = Msg::parse(&m);
        assert_eq!(m.text, "Hi there [1]");
        assert_eq!(m.links, vec!["https://q.io"]);
        assert_eq!(m.atts[0].name, "x.pdf");
    }

    #[test]
    fn identities() {
        let i = Identity { email: "*@me.org".into(), ..Identity::default() };
        assert!(i.matches("Anything@ME.org") && !i.matches("a@other.org"));
    }
}
