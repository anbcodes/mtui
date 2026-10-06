// HTML mail to plain text: block tags become line breaks, links are numbered
// and collected, entities decoded. Not a browser; just enough to read a
// newsletter in a terminal.

fn entity(name: &str) -> Option<char> {
    if let Some(n) = name.strip_prefix('#') {
        let v = match n.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => n.parse().ok()?,
        };
        return char::from_u32(v);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "rsquo" | "lsquo" => '\'',
        "ldquo" | "rdquo" => '"',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "bull" | "middot" => '·',
        "laquo" => '«',
        "raquo" => '»',
        "euro" => '€',
        "pound" => '£',
        "zwnj" | "zwj" | "shy" => '\u{200b}',
        _ => return None,
    })
}

/// Decode `&amp;`-style entities.
pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        o.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest[1..].find(';').filter(|&j| j <= 8).and_then(|j| entity(&rest[1..1 + j]).map(|c| (c, j))) {
            Some((c, j)) => {
                o.push(c);
                rest = &rest[j + 2..];
            }
            None => {
                o.push('&');
                rest = &rest[1..];
            }
        }
    }
    o.push_str(rest);
    o
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        if at == 0 || !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[from..].trim_start().strip_prefix('=')?.trim_start();
        let v = match rest.chars().next()? {
            q @ ('"' | '\'') => rest[1..].split(q).next()?.to_string(),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?.to_string(),
        };
        return Some(unescape(&v));
    }
    None
}

struct Out {
    s: String,
    quote: usize,
    links: Vec<String>,
    open: Vec<(usize, usize)>, // link index, text start
    pre: bool,
}

impl Out {
    fn trailing_newlines(&self) -> usize {
        self.s.chars().rev().take_while(|&c| c == '\n').count()
    }

    fn brk(&mut self, n: usize) {
        if self.s.is_empty() {
            return;
        }
        while self.s.ends_with(' ') {
            self.s.pop();
        }
        for _ in self.trailing_newlines()..n {
            self.s.push('\n');
        }
    }

    fn at_line_start(&self) -> bool {
        self.s.is_empty() || self.s.ends_with('\n')
    }

    fn text(&mut self, t: &str) {
        for c in t.chars() {
            if matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' | '\u{34f}' | '\u{ad}' | '\u{2060}') {
                continue;
            }
            if c == '\n' && self.pre {
                self.s.push('\n');
                continue;
            }
            if c.is_whitespace() && !self.pre {
                if !self.at_line_start() && !self.s.ends_with(' ') {
                    self.s.push(' ');
                }
                continue;
            }
            if self.at_line_start() && self.quote > 0 {
                for _ in 0..self.quote {
                    self.s.push_str("> ");
                }
            }
            self.s.push(c);
        }
    }
}

/// Plain text and the numbered links of an HTML document.
pub fn to_text(html: &str) -> (String, Vec<String>) {
    let mut o = Out { s: String::new(), quote: 0, links: Vec::new(), open: Vec::new(), pre: false };
    let mut rest = html;
    let mut skip: Option<&str> = None;
    while !rest.is_empty() {
        let Some(i) = rest.find('<') else {
            if skip.is_none() {
                o.text(&unescape(rest));
            }
            break;
        };
        if skip.is_none() {
            o.text(&unescape(&rest[..i]));
        }
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |j| &r[j + 3..]);
            continue;
        }
        let Some(j) = rest.find('>') else { break };
        let tag = &rest[1..j];
        rest = &rest[j + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if let Some(s) = skip {
            if closing && name == s {
                skip = None;
            }
            continue;
        }
        match (name.as_str(), closing) {
            ("style" | "script" | "head" | "title", false) => skip = Some(["style", "script", "head", "title"].into_iter().find(|n| *n == name).unwrap()),
            ("br", _) => o.s.push('\n'),
            ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table" | "ul" | "ol" | "section" | "article" | "header" | "footer" | "center" | "form", _) => o.brk(2),
            ("tr" | "li" | "dt" | "dd" | "hr", _) => {
                o.brk(1);
                if name == "hr" {
                    o.text("────────");
                    o.brk(1);
                } else if name == "li" && !closing {
                    o.text("• ");
                }
            }
            ("td" | "th", false) => {
                if !o.at_line_start() {
                    o.s.push_str("  ");
                }
            }
            ("pre", c) => {
                o.brk(2);
                o.pre = !c;
            }
            ("blockquote", c) => {
                o.brk(1);
                o.quote = if c { o.quote.saturating_sub(1) } else { o.quote + 1 };
            }
            ("img", false) => {
                if let Some(alt) = attr(tag, "alt").filter(|a| !a.trim().is_empty()) {
                    o.text(&format!("[{}]", alt.trim()));
                }
            }
            ("a", false) => {
                if let Some(h) = attr(tag, "href").filter(|h| h.starts_with("http") || h.starts_with("mailto:")) {
                    o.links.push(h);
                    o.open.push((o.links.len(), o.s.len()));
                }
            }
            ("a", true) => {
                if let Some((n, start)) = o.open.pop() {
                    let shown = o.s[start..].trim();
                    let url = &o.links[n - 1];
                    if !shown.is_empty() && shown != url && url.strip_prefix("mailto:") != Some(shown) {
                        o.text(&format!(" [{}]", n));
                    } else {
                        o.links.truncate(n - 1);
                    }
                }
            }
            _ => {}
        }
    }
    let mut lines: Vec<&str> = Vec::new();
    for l in o.s.lines() {
        let l = l.trim_end();
        if l.is_empty() && lines.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    (lines.join("\n"), o.links)
}

/// Remote images an HTML message would load (tracking pixels, 1-2px, left out).
pub fn remote_images(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img") {
        let start = from + i;
        let Some(len) = html[start..].find('>') else { break };
        let tag = &html[start..start + len];
        from = start + len;
        let tiny = ["width", "height"].iter().any(|a| attr(tag, a).is_some_and(|v| v.trim_end_matches("px").trim().parse::<u32>().is_ok_and(|n| n <= 2)));
        if let Some(src) = attr(tag, "src").filter(|s| s.starts_with("http") && !tiny) {
            if !out.contains(&src) {
                out.push(src);
            }
        }
    }
    out
}

/// http(s) URLs in plain text, in order, without duplicates.
pub fn find_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')' | '[' | ']')) {
        if w.starts_with("http://") || w.starts_with("https://") {
            let u = w.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
            if !out.iter().any(|x| x == u) {
                out.push(u.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let (t, l) = to_text("<html><head><style>p{}</style></head><body><p>Hello&nbsp;<b>world</b> &amp; co</p><p>See <a href=\"https://x.com/a?b=1&amp;c=2\">this page</a>.</p><ul><li>one</li><li>two</li></ul></body></html>");
        assert_eq!(t, "Hello world & co\n\nSee this page [1].\n\n• one\n• two");
        assert_eq!(l, vec!["https://x.com/a?b=1&c=2"]);
    }

    #[test]
    fn bare_links_and_quotes() {
        let (t, l) = to_text("<a href='https://a.b'>https://a.b</a><br><blockquote>quoted<br>more</blockquote>after");
        assert_eq!(t, "https://a.b\n> quoted\n> more\nafter");
        assert!(l.is_empty());
    }

    #[test]
    fn entities() {
        assert_eq!(unescape("a &lt; b &#8212; c &#x41; &bogus; &"), "a < b — c A &bogus; &");
    }

    #[test]
    fn remote() {
        let h = "<img src=\"https://a.b/x.png\" alt=hi><IMG width=1 height=1 src='https://t.rk/p.gif'><img src=\"cid:foo\"><img src=https://a.b/x.png>";
        assert_eq!(remote_images(h), vec!["https://a.b/x.png"]);
    }

    #[test]
    fn urls() {
        assert_eq!(find_urls("see https://a.b/c, and (https://d.e)."), vec!["https://a.b/c", "https://d.e"]);
    }
}
