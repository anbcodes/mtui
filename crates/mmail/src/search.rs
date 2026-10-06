// Search box syntax to a JMAP Email/query filter. Words match anywhere in a
// message; `from:` `to:` `cc:` `subject:` `in:` `is:` `has:` `before:` `after:`
// narrow it.

use crate::model::Mailbox;
use mtui::json::quote;

fn tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut inq, mut any) = (String::new(), false, false);
    for c in q.chars() {
        match c {
            '"' => {
                inq = !inq;
                any = true;
            }
            c if c.is_whitespace() && !inq => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn date(v: &str) -> Option<String> {
    let ok = v.len() == 10 && v.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    ok.then(|| format!("{}T00:00:00Z", v))
}

/// The filter JSON for `q`; searches skip Trash and Spam unless `in:` says otherwise.
pub fn filter(q: &str, boxes: &[Mailbox]) -> Result<String, String> {
    let mut c: Vec<String> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let mut scoped = false;
    for t in tokens(q) {
        let Some((k, v)) = t.split_once(':').filter(|(k, v)| !v.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic())) else {
            text.push(t);
            continue;
        };
        let v = v.trim();
        match k.to_ascii_lowercase().as_str() {
            "from" | "to" | "cc" | "bcc" | "subject" | "body" => c.push(format!("{{\"{}\":{}}}", k.to_ascii_lowercase(), quote(v))),
            "in" => {
                let l = v.to_ascii_lowercase();
                let b = boxes.iter().find(|b| b.name.to_ascii_lowercase() == l || b.role.as_deref() == Some(l.as_str()) || (l == "spam" && b.role.as_deref() == Some("junk"))).ok_or(format!("no mailbox named {}", v))?;
                c.push(format!("{{\"inMailbox\":{}}}", quote(&b.id)));
                scoped = true;
            }
            "is" => c.push(match v {
                "unread" => "{\"notKeyword\":\"$seen\"}".into(),
                "read" => "{\"hasKeyword\":\"$seen\"}".into(),
                "starred" | "flagged" => "{\"hasKeyword\":\"$flagged\"}".into(),
                "draft" => "{\"hasKeyword\":\"$draft\"}".into(),
                _ => return Err(format!("unknown is:{}", v)),
            }),
            "has" if v == "attachment" => c.push("{\"hasAttachment\":true}".into()),
            "before" => c.push(format!("{{\"before\":{}}}", quote(&date(v).ok_or("before: wants YYYY-MM-DD")?))),
            "after" => c.push(format!("{{\"after\":{}}}", quote(&date(v).ok_or("after: wants YYYY-MM-DD")?))),
            _ => text.push(t.clone()),
        }
    }
    if !text.is_empty() {
        c.push(format!("{{\"text\":{}}}", quote(&text.join(" "))));
    }
    if !scoped {
        let skip: Vec<String> = boxes.iter().filter(|b| matches!(b.role.as_deref(), Some("trash" | "junk"))).map(|b| quote(&b.id)).collect();
        if !skip.is_empty() {
            c.push(format!("{{\"inMailboxOtherThan\":[{}]}}", skip.join(",")));
        }
    }
    Ok(format!("{{\"operator\":\"AND\",\"conditions\":[{}]}}", c.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb(id: &str, name: &str, role: Option<&str>) -> Mailbox {
        Mailbox { id: id.into(), name: name.into(), role: role.map(String::from), ..Mailbox::default() }
    }

    #[test]
    fn filters() {
        let boxes = vec![mb("i", "Inbox", Some("inbox")), mb("t", "Trash", Some("trash")), mb("j", "Spam", Some("junk"))];
        let f = filter("from:bob \"big plan\" is:unread in:inbox", &boxes).unwrap();
        assert_eq!(f, "{\"operator\":\"AND\",\"conditions\":[{\"from\":\"bob\"},{\"notKeyword\":\"$seen\"},{\"inMailbox\":\"i\"},{\"text\":\"big plan\"}]}");
        let f = filter("hello", &boxes).unwrap();
        assert!(f.contains("{\"text\":\"hello\"}") && f.contains("\"inMailboxOtherThan\":[\"t\",\"j\"]"));
        assert!(filter("in:nowhere", &boxes).is_err());
        assert!(filter("before:yesterday", &boxes).is_err());
    }
}
