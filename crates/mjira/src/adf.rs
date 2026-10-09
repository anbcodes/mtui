// Jira Cloud's rich text is Atlassian Document Format (JSON); Jira Server and
// Data Center use wiki markup. Both are converted to markdown for display, and
// what you type goes back through `markup`.

use crate::timefmt;
use mtui::json::Value;

/// Attachments a document may refer to: ADF `media` nodes and wiki `!file!`
/// show them inline.
#[derive(Default)]
pub struct Media {
    pub items: Vec<MediaItem>,
}

pub struct MediaItem {
    pub name: String,
    pub url: String,
    pub image: bool,
    /// Shown inline by the text, so the attachment list needn't repeat it.
    pub used: bool,
}

impl Media {
    /// Markdown for the attachment called `alt`; an unnamed or unknown one
    /// stands for the next image not yet shown.
    fn md(&mut self, alt: &str) -> String {
        let i = self.items.iter().position(|m| !alt.is_empty() && m.name == alt).or_else(|| self.items.iter().position(|m| m.image && !m.used));
        match i {
            Some(i) => {
                let m = &mut self.items[i];
                m.used = true;
                if m.image {
                    format!("![{}]({})", m.name, m.url)
                } else {
                    format!("📎 {}", m.name)
                }
            }
            None => format!("📎 {}", if alt.is_empty() { "attachment" } else { alt }),
        }
    }
}

/// A Jira text field (description, comment body) as markdown.
pub fn field_md(v: &Value, m: &mut Media) -> String {
    match v {
        Value::Str(s) => wiki_to_md(s, m),
        Value::Obj(_) => to_markdown(v, m),
        _ => String::new(),
    }
}

pub fn to_markdown(doc: &Value, m: &mut Media) -> String {
    let mut out = String::new();
    blocks(doc.get("content").arr(), m, &mut out);
    out.trim().to_string()
}

fn blocks(nodes: &[Value], m: &mut Media, out: &mut String) {
    for n in nodes {
        block(n, m, out);
    }
}

fn prefix(t: &str, p: &str) -> String {
    t.lines().map(|l| if l.is_empty() { p.trim_end().to_string() } else { format!("{}{}", p, l) }).collect::<Vec<_>>().join("\n")
}

fn is_list(n: &Value) -> bool {
    matches!(n.get("type").str(), "bulletList" | "orderedList" | "taskList")
}

fn block(n: &Value, m: &mut Media, out: &mut String) {
    let kids = n.get("content").arr();
    match n.get("type").str() {
        "paragraph" => {
            let s = inline(kids, m);
            if !s.trim().is_empty() {
                out.push_str(s.trim_end());
                out.push_str("\n\n");
            }
        }
        "heading" => {
            let l = (n.path("attrs.level").num() as usize).clamp(1, 6);
            out.push_str(&format!("{} {}\n\n", "#".repeat(l), inline(kids, m)));
        }
        "bulletList" | "orderedList" | "taskList" => list(n, m, out),
        "codeBlock" => {
            let t: String = kids.iter().map(|k| k.get("text").str()).collect();
            out.push_str(&format!("```{}\n{}\n```\n\n", n.path("attrs.language").str(), t));
        }
        "blockquote" => {
            let mut t = String::new();
            blocks(kids, m, &mut t);
            out.push_str(&prefix(t.trim_end(), "> "));
            out.push_str("\n\n");
        }
        "panel" => {
            let mut t = String::new();
            blocks(kids, m, &mut t);
            let kind = n.path("attrs.panelType").str();
            out.push_str(&prefix(&format!("**{}**\n\n{}", kind, t.trim_end()), "> "));
            out.push_str("\n\n");
        }
        "expand" | "nestedExpand" => {
            let t = n.path("attrs.title").str();
            if !t.is_empty() {
                out.push_str(&format!("**{}**\n\n", t));
            }
            blocks(kids, m, out);
        }
        "rule" => out.push_str("---\n\n"),
        "table" => table(n, m, out),
        "mediaSingle" | "mediaGroup" => {
            for k in kids {
                out.push_str(&m.md(k.path("attrs.alt").str()));
                out.push_str("\n\n");
            }
        }
        _ if kids.is_empty() => {
            let s = inline(std::slice::from_ref(n), m);
            if !s.trim().is_empty() {
                out.push_str(s.trim_end());
                out.push_str("\n\n");
            }
        }
        _ => blocks(kids, m, out),
    }
}

fn list(n: &Value, m: &mut Media, out: &mut String) {
    let ordered = n.get("type").str() == "orderedList";
    let start = if n.path("attrs.order").is_null() { 1 } else { n.path("attrs.order").num() as usize };
    for (i, it) in n.get("content").arr().iter().enumerate() {
        let mut t = String::new();
        for c in it.get("content").arr() {
            let mut s = String::new();
            block(c, m, &mut s);
            let s = s.trim_end();
            if s.is_empty() {
                continue;
            }
            if !t.is_empty() {
                t.push_str(if is_list(c) { "\n" } else { "\n\n" });
            }
            t.push_str(s);
        }
        if it.get("type").str() == "taskItem" {
            // task items hold inline content directly
            t = inline(it.get("content").arr(), m);
        }
        let marker = if it.get("type").str() == "taskItem" {
            if it.path("attrs.state").str() == "DONE" { "- [x] ".to_string() } else { "- [ ] ".to_string() }
        } else if ordered {
            format!("{}. ", start + i)
        } else {
            "- ".to_string()
        };
        let pad = " ".repeat(if ordered { marker.len() } else { 2 });
        for (k, l) in t.lines().enumerate() {
            out.push_str(if k == 0 { &marker } else if l.is_empty() { "" } else { &pad });
            out.push_str(l);
            out.push('\n');
        }
    }
    out.push('\n');
}

fn table(n: &Value, m: &mut Media, out: &mut String) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for r in n.get("content").arr() {
        let mut cells = Vec::new();
        for c in r.get("content").arr() {
            let mut t = String::new();
            blocks(c.get("content").arr(), m, &mut t);
            cells.push(t.trim().replace('\n', " ").replace('|', "\\|"));
        }
        rows.push(cells);
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        let mut cells = r.clone();
        cells.resize(cols, String::new());
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
        if i == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(cols)));
        }
    }
    out.push('\n');
}

fn marked(t: &str, marks: &[Value]) -> String {
    let core = t.trim();
    if core.is_empty() {
        return t.to_string();
    }
    let lead = &t[..t.len() - t.trim_start().len()];
    let trail = &t[t.trim_end().len()..];
    let mut s = core.to_string();
    let has = |k: &str| marks.iter().any(|m| m.get("type").str() == k);
    if has("code") {
        s = format!("`{}`", s);
    }
    if has("em") {
        s = format!("*{}*", s);
    }
    if has("strong") {
        s = format!("**{}**", s);
    }
    if has("strike") {
        s = format!("~~{}~~", s);
    }
    if let Some(l) = marks.iter().find(|m| m.get("type").str() == "link") {
        s = format!("[{}]({})", s, l.path("attrs.href").str());
    }
    format!("{}{}{}", lead, s, trail)
}

fn inline(nodes: &[Value], m: &mut Media) -> String {
    let mut o = String::new();
    for n in nodes {
        match n.get("type").str() {
            "text" => o.push_str(&marked(n.get("text").str(), n.get("marks").arr())),
            "hardBreak" => o.push('\n'),
            "mention" => o.push_str(n.path("attrs.text").opt_str().unwrap_or("@someone")),
            "emoji" => o.push_str(n.path("attrs.text").opt_str().unwrap_or(n.path("attrs.shortName").str())),
            "inlineCard" | "blockCard" | "embedCard" => {
                let u = n.path("attrs.url").str();
                o.push_str(&format!("[{}]({})", u, u));
            }
            "status" => o.push_str(&format!("[{}]", n.path("attrs.text").str())),
            "date" => o.push_str(&timefmt::date_ms(n.path("attrs.timestamp").num() as i64)),
            "media" | "mediaInline" => o.push_str(&m.md(n.path("attrs.alt").str())),
            _ => {
                let kids = n.get("content").arr();
                if !kids.is_empty() {
                    o.push_str(&inline(kids, m));
                } else {
                    o.push_str(n.get("text").str());
                }
            }
        }
    }
    o
}

// ---- wiki markup (Jira Server / Data Center) ----

pub fn wiki_to_md(s: &str, m: &mut Media) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut code = false;
    for l in s.lines() {
        let t = l.trim_end();
        if code {
            if t.starts_with("{code") || t.starts_with("{noformat}") {
                out.push("```".into());
                code = false;
            } else {
                out.push(t.to_string());
            }
            continue;
        }
        if let Some(r) = t.strip_prefix("{code") {
            let lang = r.trim_start_matches(':').split(['}', '|']).next().unwrap_or("");
            out.push(format!("```{}", if r.starts_with(':') { lang } else { "" }));
            code = true;
            continue;
        }
        if t.starts_with("{noformat}") {
            out.push("```".into());
            code = true;
            continue;
        }
        if t == "{quote}" || t == "----" {
            out.push(if t == "----" { "---".into() } else { String::new() });
            continue;
        }
        if let Some((n, rest)) = t.strip_prefix('h').and_then(|r| r.split_once(". ")).and_then(|(n, r)| n.parse::<usize>().ok().filter(|n| (1..=6).contains(n)).map(|n| (n, r))) {
            out.push(format!("{} {}", "#".repeat(n), wiki_inline(rest, m)));
            continue;
        }
        if let Some(r) = t.strip_prefix("bq. ") {
            out.push(format!("> {}", wiki_inline(r, m)));
            continue;
        }
        let first = t.chars().next();
        let depth = t.chars().take_while(|c| Some(*c) == first && matches!(c, '*' | '#' | '-')).count();
        if depth > 0 && t[depth..].starts_with(' ') {
            let marker = if t.starts_with('#') { "1." } else { "-" };
            out.push(format!("{}{} {}", "  ".repeat(depth - 1), marker, wiki_inline(t[depth..].trim_start(), m)));
            continue;
        }
        out.push(wiki_inline(t, m));
    }
    if code {
        out.push("```".into());
    }
    out.join("\n").trim().to_string()
}

fn wiki_inline(s: &str, m: &mut Media) -> String {
    let mut o = String::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap();
        // {{mono}}
        if let Some(r) = rest.strip_prefix("{{") {
            if let Some(e) = r.find("}}") {
                o.push_str(&format!("`{}`", &r[..e]));
                i += e + 4;
                continue;
            }
        }
        // [text|url] and [url]
        if c == '[' {
            if let Some(e) = rest.find(']') {
                let inner = &rest[1..e];
                if let Some((t, u)) = inner.split_once('|') {
                    o.push_str(&format!("[{}]({})", t, u));
                    i += e + 1;
                    continue;
                }
                if inner.starts_with("http") {
                    o.push_str(&format!("[{}]({})", inner, inner));
                    i += e + 1;
                    continue;
                }
            }
        }
        // !image.png! (with optional |options)
        if c == '!' {
            if let Some(e) = rest[1..].find('!') {
                let name = rest[1..1 + e].split('|').next().unwrap_or("");
                if !name.is_empty() && !name.contains(' ') {
                    o.push_str(&m.md(name));
                    i += e + 2;
                    continue;
                }
            }
        }
        // *bold*, _italic_ and -strike- when they wrap a word
        if matches!(c, '*' | '_') && (i == 0 || s[..i].ends_with(|p: char| !p.is_alphanumeric())) {
            if let Some(e) = rest[1..].find(c) {
                let inner = &rest[1..1 + e];
                let after = rest[e + 2..].chars().next();
                if !inner.is_empty() && !inner.starts_with(' ') && !inner.ends_with(' ') && after.is_none_or(|a| !a.is_alphanumeric()) {
                    o.push_str(&if c == '*' { format!("**{}**", inner) } else { format!("*{}*", inner) });
                    i += e + 2;
                    continue;
                }
            }
        }
        o.push(c);
        i += c.len_utf8();
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtui::json;

    fn md(j: &str) -> String {
        to_markdown(&json::parse(j).unwrap(), &mut Media::default())
    }

    #[test]
    fn paragraphs_and_marks() {
        let s = md(r#"{"type":"doc","content":[{"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Plan"}]},{"type":"paragraph","content":[{"type":"text","text":"Do "},{"type":"text","text":"this ","marks":[{"type":"strong"}]},{"type":"text","text":"now","marks":[{"type":"link","attrs":{"href":"http://x"}}]},{"type":"hardBreak"},{"type":"mention","attrs":{"text":"@Ann"}},{"type":"text","text":" ok"}]}]}"#);
        assert_eq!(s, "## Plan\n\nDo **this** [now](http://x)\n@Ann ok");
    }

    #[test]
    fn lists_code_tables() {
        let s = md(r#"{"type":"doc","content":[{"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"a"}]},{"type":"orderedList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"b"}]}]}]}]},{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"c"}]}]}]},{"type":"codeBlock","attrs":{"language":"rs"},"content":[{"type":"text","text":"let x;"}]},{"type":"table","content":[{"type":"tableRow","content":[{"type":"tableHeader","content":[{"type":"paragraph","content":[{"type":"text","text":"h"}]}]}]},{"type":"tableRow","content":[{"type":"tableCell","content":[{"type":"paragraph","content":[{"type":"text","text":"v"}]}]}]}]}]}"#);
        assert_eq!(s, "- a\n  1. b\n- c\n\n```rs\nlet x;\n```\n\n| h |\n| --- |\n| v |");
    }

    #[test]
    fn media_maps_to_attachments() {
        let mut m = Media { items: vec![MediaItem { name: "a.png".into(), url: "http://h/a".into(), image: true, used: false }, MediaItem { name: "b.pdf".into(), url: "http://h/b".into(), image: false, used: false }] };
        let d = json::parse(r#"{"type":"doc","content":[{"type":"mediaSingle","content":[{"type":"media","attrs":{"id":"x","alt":"a.png"}}]},{"type":"mediaSingle","content":[{"type":"media","attrs":{"id":"y","alt":"b.pdf"}}]}]}"#).unwrap();
        assert_eq!(to_markdown(&d, &mut m), "![a.png](http://h/a)\n\n📎 b.pdf");
        assert!(m.items.iter().all(|i| i.used));
    }

    #[test]
    fn wiki() {
        let mut m = Media::default();
        let s = wiki_to_md("h2. Title\n* one\n** two\n# n1\nSome *bold* and _it_ {{mono}} [link|http://x]\n{code:java}\nint x;\n{code}", &mut m);
        assert_eq!(s, "## Title\n- one\n  - two\n1. n1\nSome **bold** and *it* `mono` [link](http://x)\n```java\nint x;\n```");
        assert_eq!(wiki_to_md("snake_case_name and 2*3*4", &mut m), "snake_case_name and 2*3*4");
    }
}
