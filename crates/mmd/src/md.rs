// Markdown to HTML: CommonMark's everyday constructs plus the GitHub extras
// people actually write (tables, task lists, strikethrough, bare links,
// alerts). Every block carries `data-line`, the 1-based source line it
// starts on, so a preview can be scrolled to match an editor.

use mtui::syntax;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default, Clone)]
pub struct Options {
    /// Single newlines inside a paragraph break the line (as in GitHub comments).
    pub hard_breaks: bool,
    /// Embed images found relative to this directory as data: URIs, so the
    /// page stands alone.
    pub embed_from: Option<PathBuf>,
}

pub struct Heading {
    pub level: u8,
    pub id: String,
    pub text: String,
}

pub struct Doc {
    pub html: String,
    pub headings: Vec<Heading>,
    /// The first level-1 heading, if any.
    pub title: Option<String>,
}

pub fn render(src: &str, o: &Options) -> Doc {
    let mut lines: Vec<Ln> = Vec::new();
    for (i, l) in src.lines().enumerate() {
        lines.push(Ln { n: i + 1, s: expand_tabs(l) });
    }
    let mut r = R { refs: HashMap::new(), ids: HashMap::new(), headings: Vec::new(), o: o.clone() };
    let lines = r.take_refs(lines);
    let mut html = String::new();
    r.blocks(&lines, &mut html, false);
    let title = r.headings.iter().find(|h| h.level == 1).map(|h| h.text.clone());
    Doc { html, headings: r.headings, title }
}

fn expand_tabs(l: &str) -> String {
    if !l.contains('\t') {
        return l.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for c in l.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

#[derive(Clone, Debug)]
struct Ln {
    n: usize,
    s: String,
}

struct R {
    refs: HashMap<String, (String, Option<String>)>,
    ids: HashMap<String, usize>,
    headings: Vec<Heading>,
    o: Options,
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

fn indent(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn strip_cols(s: &str, n: usize) -> String {
    let i = indent(s).min(n);
    s[i..].to_string()
}

fn fence_open(s: &str) -> Option<(char, usize, String)> {
    if indent(s) > 3 {
        return None;
    }
    let t = s.trim_start();
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    let info = t[n..].trim();
    (n >= 3 && !(c == '`' && info.contains('`'))).then(|| (c, n, info.to_string()))
}

fn is_hr(s: &str) -> bool {
    if indent(s) > 3 {
        return false;
    }
    let t: String = s.chars().filter(|c| *c != ' ').collect();
    let Some(c) = t.chars().next() else { return false };
    matches!(c, '-' | '*' | '_') && t.len() >= 3 && t.chars().all(|x| x == c)
}

fn atx(s: &str) -> Option<(u8, String)> {
    if indent(s) > 3 {
        return None;
    }
    let t = s.trim_start();
    let n = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &t[n..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let rest = rest.trim();
    let stripped = rest.trim_end_matches('#');
    let text = if stripped.is_empty() || stripped.ends_with(' ') { stripped.trim() } else { rest };
    Some((n as u8, text.to_string()))
}

fn quote_strip(s: &str) -> Option<String> {
    if indent(s) > 3 {
        return None;
    }
    let t = s.trim_start().strip_prefix('>')?;
    Some(t.strip_prefix(' ').unwrap_or(t).to_string())
}

#[derive(Clone, Copy)]
struct Marker {
    ordered: bool,
    ch: char,
    num: usize,
    /// Column where the item's content starts.
    content: usize,
    empty: bool,
}

fn list_marker(s: &str) -> Option<Marker> {
    let ind = indent(s);
    if ind > 3 {
        return None;
    }
    let t = &s[ind..];
    let (ordered, ch, num, w) = match t.chars().next()? {
        c @ ('-' | '+' | '*') => (false, c, 0, 1),
        c if c.is_ascii_digit() => {
            let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
            let delim = t[d..].chars().next()?;
            if d > 9 || !matches!(delim, '.' | ')') {
                return None;
            }
            (true, delim, t[..d].parse().ok()?, d + 1)
        }
        _ => return None,
    };
    let rest = &t[w..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let spaces = indent(rest);
    let empty = rest.trim().is_empty();
    let sp = if empty || spaces > 4 { 1 } else { spaces };
    Some(Marker { ordered, ch, num, content: ind + w + sp, empty })
}

const BLOCK_TAGS: &[&str] = &[
    "address", "article", "aside", "blockquote", "body", "caption", "center", "col", "colgroup", "dd", "details", "dialog", "dir", "div", "dl", "dt", "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hr", "html", "iframe", "legend", "li", "link", "main", "menu", "nav", "ol", "optgroup", "option", "p", "param", "section", "source", "summary", "table", "tbody", "td", "tfoot", "th", "thead", "title", "tr", "track", "ul", "pre", "script", "style", "textarea", "picture",
];

fn html_block_start(s: &str) -> bool {
    if indent(s) > 3 {
        return false;
    }
    let t = s.trim_start();
    if t.starts_with("<!--") {
        return true;
    }
    let Some(r) = t.strip_prefix('<') else { return false };
    let r = r.strip_prefix('/').unwrap_or(r);
    let name: String = r.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    BLOCK_TAGS.contains(&name.as_str()) && (r.len() == name.len() || r[name.len()..].starts_with([' ', '>', '/']))
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

fn split_row(s: &str) -> Vec<String> {
    let t = s.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    let mut tick = false;
    for c in t.chars() {
        match c {
            '\\' if !esc => {
                esc = true;
                cur.push(c);
                continue;
            }
            '`' if !esc => tick = !tick,
            '|' if !esc && !tick => {
                cells.push(cur.trim().to_string());
                cur = String::new();
                esc = false;
                continue;
            }
            _ => {}
        }
        esc = false;
        cur.push(c);
    }
    if !cur.trim().is_empty() || cells.is_empty() {
        cells.push(cur.trim().to_string());
    }
    cells.into_iter().map(|c| c.replace("\\|", "|")).collect()
}

fn table_delim(s: &str) -> Option<Vec<Align>> {
    if !s.contains('-') || indent(s) > 3 {
        return None;
    }
    let cells = split_row(s);
    let mut out = Vec::new();
    for c in &cells {
        let l = c.starts_with(':');
        let r = c.ends_with(':');
        let core = c.trim_matches(':');
        if core.is_empty() || !core.chars().all(|x| x == '-') {
            return None;
        }
        out.push(match (l, r) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            _ => Align::None,
        });
    }
    Some(out)
}

fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if (c == ' ' || c == '-') && !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_matches('-').to_string()
}

fn strip_tags(h: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in h.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

fn norm_label(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn parse_ref(s: &str) -> Option<(String, String, Option<String>)> {
    if indent(s) > 3 {
        return None;
    }
    let t = s.trim();
    let r = t.strip_prefix('[')?;
    let (label, rest) = r.split_once("]:")?;
    if label.is_empty() || label.contains(['[', ']']) || label.starts_with('^') {
        return None;
    }
    let rest = rest.trim();
    let (url, tail) = if let Some(r) = rest.strip_prefix('<') {
        let e = r.find('>')?;
        (&r[..e], r[e + 1..].trim())
    } else {
        let e = rest.find(char::is_whitespace).unwrap_or(rest.len());
        (&rest[..e], rest[e..].trim())
    };
    if url.is_empty() {
        return None;
    }
    let title = if tail.is_empty() {
        None
    } else {
        let b = tail.as_bytes();
        let (o, c) = (b[0], *b.last()?);
        if tail.len() >= 2 && ((o == b'"' && c == b'"') || (o == b'\'' && c == b'\'') || (o == b'(' && c == b')')) {
            Some(tail[1..tail.len() - 1].to_string())
        } else {
            return None;
        }
    };
    Some((norm_label(label), url.to_string(), title))
}

impl R {
    /// Pull link reference definitions out of the text (outside code fences).
    fn take_refs(&mut self, lines: Vec<Ln>) -> Vec<Ln> {
        let mut out = Vec::new();
        let mut fence: Option<(char, usize)> = None;
        let mut prev_blank = true;
        for l in lines {
            if let Some((c, n)) = fence {
                if fence_open(&l.s).is_some_and(|(c2, n2, info)| c2 == c && n2 >= n && info.is_empty()) {
                    fence = None;
                }
                out.push(l);
                continue;
            }
            if let Some((c, n, _)) = fence_open(&l.s) {
                fence = Some((c, n));
            } else if prev_blank || out.last().is_some_and(|p: &Ln| parse_ref(&p.s).is_some()) {
                if let Some((k, u, t)) = parse_ref(&l.s) {
                    self.refs.entry(k).or_insert((u, t));
                    continue;
                }
            }
            prev_blank = is_blank(&l.s);
            out.push(l);
        }
        out
    }

    fn unique_id(&mut self, base: &str) -> String {
        let base = if base.is_empty() { "section".to_string() } else { base.to_string() };
        let n = self.ids.entry(base.clone()).or_insert(0);
        let id = if *n == 0 { base.clone() } else { format!("{}-{}", base, n) };
        *n += 1;
        id
    }

    fn heading(&mut self, level: u8, text: &str, n: usize, out: &mut String) {
        let inner = self.inline(text);
        let plain = strip_tags(&inner);
        let id = self.unique_id(&slug(&plain));
        out.push_str(&format!("<h{l} id=\"{id}\" data-line=\"{n}\"><a class=\"anchor\" href=\"#{id}\">#</a>{inner}</h{l}>\n", l = level, id = id, n = n, inner = inner));
        self.headings.push(Heading { level, id, text: plain });
    }

    fn blocks(&mut self, lines: &[Ln], out: &mut String, tight: bool) {
        let mut i = 0;
        while i < lines.len() {
            let l = &lines[i];
            if is_blank(&l.s) {
                i += 1;
                continue;
            }
            // fenced code
            if let Some((c, n, info)) = fence_open(&l.s) {
                let ind = indent(&l.s);
                let mut code: Vec<String> = Vec::new();
                i += 1;
                while i < lines.len() {
                    if fence_open(&lines[i].s).is_some_and(|(c2, n2, inf)| c2 == c && n2 >= n && inf.is_empty()) {
                        i += 1;
                        break;
                    }
                    code.push(strip_cols(&lines[i].s, ind));
                    i += 1;
                }
                self.code_block(&code.join("\n"), info.split_whitespace().next().unwrap_or(""), l.n, out);
                continue;
            }
            if let Some((level, text)) = atx(&l.s) {
                self.heading(level, &text, l.n, out);
                i += 1;
                continue;
            }
            if is_hr(&l.s) {
                out.push_str(&format!("<hr data-line=\"{}\">\n", l.n));
                i += 1;
                continue;
            }
            if quote_strip(&l.s).is_some() {
                i = self.quote(lines, i, out);
                continue;
            }
            if list_marker(&l.s).is_some() {
                i = self.list(lines, i, out);
                continue;
            }
            if indent(&l.s) >= 4 {
                let n0 = l.n;
                let mut code: Vec<String> = Vec::new();
                while i < lines.len() && (indent(&lines[i].s) >= 4 || is_blank(&lines[i].s)) {
                    code.push(strip_cols(&lines[i].s, 4));
                    i += 1;
                }
                while code.last().is_some_and(|c| c.trim().is_empty()) {
                    code.pop();
                }
                self.code_block(&code.join("\n"), "", n0, out);
                continue;
            }
            if html_block_start(&l.s) {
                let n0 = l.n;
                let t = l.s.trim_start().to_lowercase();
                let end: Option<&str> = if t.starts_with("<!--") { Some("-->") } else if t.starts_with("<pre") { Some("</pre>") } else { None };
                let mut buf = Vec::new();
                while i < lines.len() {
                    let done = end.is_some_and(|e| lines[i].s.to_lowercase().contains(e));
                    if end.is_none() && is_blank(&lines[i].s) {
                        break;
                    }
                    buf.push(lines[i].s.as_str());
                    i += 1;
                    if done {
                        break;
                    }
                }
                let raw = buf.join("\n");
                if raw.trim_start().starts_with("<!--") {
                    out.push_str(&format!("{}\n", raw));
                } else {
                    out.push_str(&format!("<div class=\"html\" data-line=\"{}\">{}</div>\n", n0, safe_html(&raw)));
                }
                continue;
            }
            if l.s.contains('|') && i + 1 < lines.len() {
                if let Some(al) = table_delim(&lines[i + 1].s) {
                    let head = split_row(&l.s);
                    if head.len() == al.len() {
                        i = self.table(lines, i, al, out);
                        continue;
                    }
                }
            }
            i = self.paragraph(lines, i, out, tight);
        }
    }

    fn paragraph(&mut self, lines: &[Ln], start: usize, out: &mut String, tight: bool) -> usize {
        let mut i = start;
        let mut buf: Vec<&str> = Vec::new();
        while i < lines.len() {
            let s = &lines[i].s;
            if is_blank(s) {
                break;
            }
            if i > start {
                if fence_open(s).is_some() || atx(s).is_some() || is_hr(s) || quote_strip(s).is_some() || html_block_start(s) && !s.trim_start().starts_with("</") {
                    break;
                }
                if let Some(m) = list_marker(s) {
                    if !m.empty && (!m.ordered || m.num == 1) {
                        break;
                    }
                }
                // setext underline for what we have so far
                let u = s.trim();
                if indent(s) <= 3 && !u.is_empty() && (u.chars().all(|c| c == '=') || u.chars().all(|c| c == '-')) {
                    let level = if u.starts_with('=') { 1 } else { 2 };
                    let text = buf.join("\n");
                    self.heading(level, text.trim(), lines[start].n, out);
                    return i + 1;
                }
            }
            buf.push(s.trim_start());
            i += 1;
        }
        let text = buf.join("\n");
        let inner = self.inline(text.trim_end());
        if tight {
            out.push_str(&inner);
            out.push('\n');
        } else {
            out.push_str(&format!("<p data-line=\"{}\">{}</p>\n", lines[start].n, inner));
        }
        i
    }

    fn quote(&mut self, lines: &[Ln], start: usize, out: &mut String) -> usize {
        let mut i = start;
        let mut inner: Vec<Ln> = Vec::new();
        while i < lines.len() {
            let l = &lines[i];
            if let Some(s) = quote_strip(&l.s) {
                inner.push(Ln { n: l.n, s });
            } else if !is_blank(&l.s) && inner.last().is_some_and(|p| !is_blank(&p.s)) && fence_open(&l.s).is_none() && atx(&l.s).is_none() && !is_hr(&l.s) && list_marker(&l.s).is_none() {
                // lazy continuation of a paragraph
                inner.push(Ln { n: l.n, s: l.s.clone() });
            } else {
                break;
            }
            i += 1;
        }
        // GitHub alerts: > [!NOTE]
        let kind = inner.first().and_then(|f| {
            let t = f.s.trim();
            let k = t.strip_prefix("[!")?.strip_suffix(']')?;
            matches!(k, "NOTE" | "TIP" | "IMPORTANT" | "WARNING" | "CAUTION").then(|| k.to_string())
        });
        let n = lines[start].n;
        if let Some(k) = kind {
            inner.remove(0);
            let mut body = String::new();
            self.blocks(&inner, &mut body, false);
            let title = format!("{}{}", &k[..1], k[1..].to_lowercase());
            out.push_str(&format!("<div class=\"alert alert-{}\" data-line=\"{}\"><p class=\"alert-title\">{}</p>\n{}</div>\n", k.to_lowercase(), n, title, body));
        } else {
            let mut body = String::new();
            self.blocks(&inner, &mut body, false);
            out.push_str(&format!("<blockquote data-line=\"{}\">\n{}</blockquote>\n", n, body));
        }
        i
    }

    fn list(&mut self, lines: &[Ln], start: usize, out: &mut String) -> usize {
        let first = list_marker(&lines[start].s).unwrap();
        let mut i = start;
        let mut items: Vec<(usize, Vec<Ln>)> = Vec::new();
        let mut loose = false;
        while i < lines.len() {
            let Some(m) = list_marker(&lines[i].s).filter(|m| m.ordered == first.ordered && m.ch == first.ch) else { break };
            if is_hr(&lines[i].s) {
                break;
            }
            let ind = indent(&lines[i].s);
            let n0 = lines[i].n;
            let mut body: Vec<Ln> = vec![Ln { n: n0, s: lines[i].s[(m.content.min(lines[i].s.len())).max(ind)..].to_string() }];
            if m.empty {
                body[0].s.clear();
            }
            i += 1;
            while i < lines.len() {
                let l = &lines[i];
                if is_blank(&l.s) {
                    let mut j = i;
                    while j < lines.len() && is_blank(&lines[j].s) {
                        j += 1;
                    }
                    if j < lines.len() && indent(&lines[j].s) >= m.content && !(m.empty && body.len() == 1) {
                        while i < j {
                            body.push(Ln { n: lines[i].n, s: String::new() });
                            i += 1;
                        }
                        continue;
                    }
                    break;
                }
                if indent(&l.s) >= m.content {
                    body.push(Ln { n: l.n, s: strip_cols(&l.s, m.content) });
                    i += 1;
                } else if body.last().is_some_and(|p| !is_blank(&p.s)) && list_marker(&l.s).is_none() && fence_open(&l.s).is_none() && atx(&l.s).is_none() && !is_hr(&l.s) && quote_strip(&l.s).is_none() && !html_block_start(&l.s) {
                    body.push(Ln { n: l.n, s: l.s.trim_start().to_string() });
                    i += 1;
                } else {
                    break;
                }
            }
            // a blank line inside an item, before more than a nested list or code, makes the list loose
            let mut fence = false;
            for k in 0..body.len().saturating_sub(1) {
                if fence_open(&body[k].s).is_some() {
                    fence = !fence;
                }
                if !fence && is_blank(&body[k].s) && !is_blank(&body[k + 1].s) && indent(&body[k + 1].s) == 0 && list_marker(&body[k + 1].s).is_none() && k > 0 {
                    loose = true;
                }
            }
            while body.last().is_some_and(|b| is_blank(&b.s)) {
                body.pop();
            }
            if body.is_empty() {
                body.push(Ln { n: n0, s: String::new() });
            }
            items.push((n0, body));
            // blank lines between items
            let mut j = i;
            while j < lines.len() && is_blank(&lines[j].s) {
                j += 1;
            }
            if j < lines.len() && list_marker(&lines[j].s).is_some_and(|m2| m2.ordered == first.ordered && m2.ch == first.ch) && !is_hr(&lines[j].s) {
                loose |= j > i;
                i = j;
            } else {
                break;
            }
        }
        let tag = if first.ordered { "ol" } else { "ul" };
        let startattr = if first.ordered && first.num != 1 { format!(" start=\"{}\"", first.num) } else { String::new() };
        let has_tasks = items.iter().any(|(_, b)| task(&b[0].s).is_some());
        out.push_str(&format!("<{} data-line=\"{}\"{}{}>\n", tag, lines[start].n, startattr, if has_tasks { " class=\"tasks\"" } else { "" }));
        for (n0, mut body) in items {
            let mut li = String::new();
            if let Some((done, rest)) = task(&body[0].s) {
                li.push_str(&format!("<input type=\"checkbox\" disabled{}> ", if done { " checked" } else { "" }));
                body[0].s = rest;
            }
            self.blocks(&body, &mut li, !loose);
            out.push_str(&format!("<li data-line=\"{}\">{}</li>\n", n0, li.trim_end()));
        }
        out.push_str(&format!("</{}>\n", tag));
        i
    }

    fn table(&mut self, lines: &[Ln], start: usize, al: Vec<Align>, out: &mut String) -> usize {
        let cell = |this: &mut R, tag: &str, text: &str, a: Align| {
            let style = match a {
                Align::Left => " style=\"text-align:left\"",
                Align::Center => " style=\"text-align:center\"",
                Align::Right => " style=\"text-align:right\"",
                Align::None => "",
            };
            format!("<{t}{s}>{c}</{t}>", t = tag, s = style, c = this.inline(text))
        };
        out.push_str(&format!("<table data-line=\"{}\">\n<thead><tr>", lines[start].n));
        for (k, c) in split_row(&lines[start].s).iter().enumerate() {
            out.push_str(&cell(self, "th", c, al[k]));
        }
        out.push_str("</tr></thead>\n<tbody>\n");
        let mut i = start + 2;
        while i < lines.len() && !is_blank(&lines[i].s) && lines[i].s.contains('|') && fence_open(&lines[i].s).is_none() {
            out.push_str("<tr>");
            let mut cells = split_row(&lines[i].s);
            cells.resize(al.len(), String::new());
            for (k, c) in cells.iter().enumerate().take(al.len()) {
                out.push_str(&cell(self, "td", c, al[k]));
            }
            out.push_str("</tr>\n");
            i += 1;
        }
        out.push_str("</tbody></table>\n");
        i
    }

    fn code_block(&mut self, code: &str, lang: &str, n: usize, out: &mut String) {
        let body = highlight(code, lang);
        out.push_str(&format!("<pre data-line=\"{}\"{}><code>{}</code></pre>\n", n, if lang.is_empty() { String::new() } else { format!(" data-lang=\"{}\"", esc(lang)) }, body));
    }
}

/// `- [ ] x` / `- [x] x`: (checked, rest).
fn task(s: &str) -> Option<(bool, String)> {
    let r = s.strip_prefix('[')?;
    let c = r.chars().next()?;
    let r = &r[c.len_utf8()..];
    let r = r.strip_prefix("] ")?;
    match c {
        ' ' => Some((false, r.to_string())),
        'x' | 'X' => Some((true, r.to_string())),
        _ => None,
    }
}

/// Code with syntax-highlighting spans (`<span class="k1">`..) for languages we know.
fn highlight(code: &str, lang: &str) -> String {
    let Some(lg) = (!lang.is_empty()).then(|| syntax::by_name(lang)).flatten() else { return esc(code) };
    let mut out = String::new();
    let mut st = syntax::State::default();
    let mut kinds: Vec<u8> = Vec::new();
    for (ln, line) in code.split('\n').enumerate() {
        if ln > 0 {
            out.push('\n');
        }
        st = syntax::highlight(lg, line, st, &mut kinds);
        let mut cur = 0u8;
        let mut open = false;
        for (bi, ch) in line.char_indices() {
            let k = *kinds.get(bi).unwrap_or(&0);
            if k != cur || (!open && k != 0) {
                if open {
                    out.push_str("</span>");
                    open = false;
                }
                if k != 0 {
                    out.push_str(&format!("<span class=\"k{}\">", k));
                    open = true;
                }
                cur = k;
            }
            let mut b = [0u8; 4];
            out.push_str(&esc(ch.encode_utf8(&mut b)));
        }
        if open {
            out.push_str("</span>");
        }
    }
    out
}

// ---- inline HTML safety ----

const BAD_TAGS: &[&str] = &["script", "style", "iframe", "object", "embed", "form", "meta", "link", "base", "frame", "frameset", "applet", "textarea", "input", "button", "select"];

/// Length of an HTML tag or comment at the start of `s` that is safe to pass
/// through, if it is one.
fn html_tag(s: &str) -> Option<usize> {
    if s.starts_with("<!--") {
        return s.find("-->").map(|e| e + 3);
    }
    let r = s.strip_prefix('<')?;
    let body = r.strip_prefix('/').unwrap_or(r);
    let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    if name.is_empty() || !name.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    // find the closing '>' outside quotes
    let mut q: Option<char> = None;
    let mut end = None;
    for (i, c) in r.char_indices() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (None, '"' | '\'') => q = Some(c),
            (None, '>') => {
                end = Some(i + 2);
                break;
            }
            (None, '<') => return None,
            _ => {}
        }
    }
    let end = end?;
    let tag = s[..end].to_lowercase();
    if BAD_TAGS.contains(&name.to_lowercase().as_str()) {
        return None;
    }
    // event handlers and script URLs
    let attrs = &tag[1 + name.len()..];
    let compact: String = attrs.chars().filter(|c| !c.is_whitespace()).collect();
    if attrs.split(|c: char| c.is_whitespace() || c == '"' || c == '\'').any(|w| w.starts_with("on") && w.contains('=')) || compact.contains("javascript:") || compact.contains("vbscript:") || compact.contains("data:text") {
        return None;
    }
    Some(end)
}

fn safe_html(raw: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        let rest = &raw[i..];
        let c = rest.chars().next().unwrap();
        if c == '<' {
            match html_tag(rest) {
                Some(n) => {
                    out.push_str(&rest[..n]);
                    i += n;
                }
                None => {
                    out.push_str("&lt;");
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn safe_url(u: &str, image: bool) -> String {
    let t: String = u.trim().chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect::<String>().to_lowercase();
    if t.starts_with("javascript:") || t.starts_with("vbscript:") || (t.starts_with("data:") && !(image && t.starts_with("data:image/") && !t.starts_with("data:image/svg"))) {
        return "#".into();
    }
    u.trim().to_string()
}

fn mime_of(p: &str) -> Option<&'static str> {
    match p.rsplit('.').next()?.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

// ---- inline ----

enum Node {
    Raw(String),
    D { ch: char, n: usize, orig: usize, open: bool, close: bool, pre: String, post: String },
}

fn is_punct(c: char) -> bool {
    !c.is_alphanumeric() && !c.is_whitespace()
}

fn find_close_bracket(s: &str, from: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 1;
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'`' => {
                let n = s[i..].bytes().take_while(|&c| c == b'`').count();
                if let Some(e) = s[i + n..].find(&"`".repeat(n)) {
                    i += n + e + n - 1;
                } else {
                    i += n - 1;
                }
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `(dest "title")` starting just after the `(`: (dest, title, bytes consumed incl. `)`).
fn dest_title(s: &str) -> Option<(String, Option<String>, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    let ws = |i: &mut usize| {
        while *i < b.len() && (b[*i] == b' ' || b[*i] == b'\n') {
            *i += 1;
        }
    };
    ws(&mut i);
    let dest;
    if b.get(i) == Some(&b'<') {
        let e = s[i + 1..].find('>')?;
        dest = s[i + 1..i + 1 + e].to_string();
        i += e + 2;
    } else {
        let st = i;
        let mut depth = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 1,
                b' ' | b'\n' => break,
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        dest = s[st..i.min(s.len())].to_string();
    }
    ws(&mut i);
    let mut title = None;
    if let Some(&q) = b.get(i).filter(|c| matches!(c, b'"' | b'\'' | b'(')) {
        let close = if q == b'(' { b')' } else { q };
        let e = s[i + 1..].find(close as char)?;
        title = Some(s[i + 1..i + 1 + e].to_string());
        i += e + 2;
        ws(&mut i);
    }
    (b.get(i) == Some(&b')')).then_some((dest, title, i + 1))
}

fn unescape(s: &str) -> String {
    let mut o = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            o.push(it.next().unwrap());
        } else {
            o.push(c);
        }
    }
    o
}

impl R {
    fn image_src(&self, url: &str) -> String {
        let u = safe_url(&unescape(url), true);
        if let (Some(base), false) = (&self.o.embed_from, u.contains("://") || u.starts_with("data:") || u.starts_with('/')) {
            let path = base.join(u.split(['#', '?']).next().unwrap_or(""));
            if let (Some(mime), Ok(bytes)) = (mime_of(&u), std::fs::read(&path)) {
                if bytes.len() < 8 << 20 {
                    return format!("data:{};base64,{}", mime, mtui::base64::encode(&bytes));
                }
            }
        }
        u
    }

    fn inline(&self, s: &str) -> String {
        let mut nodes: Vec<Node> = Vec::new();
        let mut buf = String::new();
        let b = s.as_bytes();
        let mut i = 0;
        macro_rules! flush {
            () => {
                if !buf.is_empty() {
                    nodes.push(Node::Raw(std::mem::take(&mut buf)));
                }
            };
        }
        while i < s.len() {
            let rest = &s[i..];
            let c = rest.chars().next().unwrap();
            match c {
                '\\' => {
                    match rest[1..].chars().next() {
                        Some('\n') => {
                            flush!();
                            nodes.push(Node::Raw("<br>\n".into()));
                            i += 2;
                        }
                        Some(n) if n.is_ascii_punctuation() => {
                            buf.push_str(&esc(&n.to_string()));
                            i += 2;
                        }
                        _ => {
                            buf.push('\\');
                            i += 1;
                        }
                    }
                    continue;
                }
                '`' => {
                    let n = rest.bytes().take_while(|&x| x == b'`').count();
                    let fence = "`".repeat(n);
                    let mut j = i + n;
                    let mut found = None;
                    while let Some(e) = s[j..].find(&fence) {
                        let at = j + e;
                        let run = s[at..].bytes().take_while(|&x| x == b'`').count();
                        if run == n {
                            found = Some(at);
                            break;
                        }
                        j = at + run;
                    }
                    if let Some(e) = found {
                        let mut code = s[i + n..e].replace('\n', " ");
                        if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty() {
                            code = code[1..code.len() - 1].to_string();
                        }
                        flush!();
                        nodes.push(Node::Raw(format!("<code>{}</code>", esc(&code))));
                        i = e + n;
                    } else {
                        buf.push_str(&fence);
                        i += n;
                    }
                    continue;
                }
                '<' => {
                    // autolink
                    if let Some(e) = rest.find('>') {
                        let inner = &rest[1..e];
                        let is_url = inner.contains("://") && !inner.contains(char::is_whitespace) || inner.starts_with("mailto:");
                        let is_mail = !is_url && inner.contains('@') && !inner.contains(char::is_whitespace) && !inner.contains('<');
                        if is_url || is_mail {
                            flush!();
                            let href = if is_mail { format!("mailto:{}", inner) } else { safe_url(inner, false) };
                            nodes.push(Node::Raw(format!("<a href=\"{}\">{}</a>", esc(&href), esc(inner))));
                            i += e + 1;
                            continue;
                        }
                    }
                    if let Some(n) = html_tag(rest) {
                        flush!();
                        nodes.push(Node::Raw(rest[..n].to_string()));
                        i += n;
                    } else {
                        buf.push_str("&lt;");
                        i += 1;
                    }
                    continue;
                }
                '!' | '[' => {
                    let img = c == '!';
                    let open = if img { i + 1 } else { i };
                    if b.get(open) == Some(&b'[') {
                        if let Some((html, used)) = self.link(s, open, img) {
                            flush!();
                            nodes.push(Node::Raw(html));
                            i = used;
                            continue;
                        }
                    }
                    buf.push(c);
                    i += 1;
                    continue;
                }
                '*' | '_' | '~' => {
                    let n = rest.bytes().take_while(|&x| x == c as u8).count();
                    let prev = s[..i].chars().next_back().unwrap_or(' ');
                    let next = s[i + n..].chars().next().unwrap_or(' ');
                    let (ws_n, ws_p) = (next.is_whitespace(), prev.is_whitespace());
                    let left = !ws_n && (!is_punct(next) || ws_p || is_punct(prev));
                    let right = !ws_p && (!is_punct(prev) || ws_n || is_punct(next));
                    let (open, close) = if c == '_' { (left && (!right || is_punct(prev)), right && (!left || is_punct(next))) } else { (left, right) };
                    if c == '~' && n != 2 {
                        buf.push_str(&rest[..n]);
                    } else {
                        flush!();
                        nodes.push(Node::D { ch: c, n, orig: n, open, close, pre: String::new(), post: String::new() });
                    }
                    i += n;
                    continue;
                }
                '&' => {
                    let end = rest.find(';').filter(|&e| e > 1 && e < 12 && rest[1..e].chars().all(|x| x.is_ascii_alphanumeric() || x == '#'));
                    match end {
                        Some(e) => {
                            buf.push_str(&rest[..=e]);
                            i += e + 1;
                        }
                        None => {
                            buf.push_str("&amp;");
                            i += 1;
                        }
                    }
                    continue;
                }
                '\n' => {
                    let spaces = buf.len() - buf.trim_end_matches(' ').len();
                    buf.truncate(buf.len() - spaces);
                    if spaces >= 2 || self.o.hard_breaks {
                        buf.push_str("<br>");
                    }
                    buf.push('\n');
                    i += 1;
                    while s[i..].starts_with(' ') {
                        i += 1;
                    }
                    continue;
                }
                'h' if (rest.starts_with("http://") || rest.starts_with("https://")) && !s[..i].chars().next_back().is_some_and(|p| p.is_alphanumeric()) => {
                    let e = rest.find(|x: char| x.is_whitespace() || x == '<').unwrap_or(rest.len());
                    let mut url = &rest[..e];
                    while let Some(l) = url.chars().last() {
                        let unbalanced = l == ')' && url.matches(')').count() > url.matches('(').count();
                        if matches!(l, '.' | ',' | ':' | ';' | '!' | '?' | '*' | '_' | '~' | '\'' | '"') || unbalanced {
                            url = &url[..url.len() - l.len_utf8()];
                        } else {
                            break;
                        }
                    }
                    if url.len() > 8 {
                        flush!();
                        nodes.push(Node::Raw(format!("<a href=\"{}\">{}</a>", esc(url), esc(url))));
                        i += url.len();
                        continue;
                    }
                }
                _ => {}
            }
            buf.push_str(&esc(&c.to_string()));
            i += c.len_utf8();
        }
        flush!();
        resolve(nodes)
    }

    /// A link or image whose `[` is at `open`: (html, index after it).
    fn link(&self, s: &str, open: usize, img: bool) -> Option<(String, usize)> {
        let close = find_close_bracket(s, open + 1)?;
        let text = &s[open + 1..close];
        let after = &s[close + 1..];
        let (dest, title, used) = if let Some(r) = after.strip_prefix('(') {
            let (d, t, n) = dest_title(r)?;
            (d, t, close + 1 + 1 + n)
        } else {
            let (label, used) = if let Some(r) = after.strip_prefix('[') {
                let e = r.find(']')?;
                (if e == 0 { text.to_string() } else { r[..e].to_string() }, close + 1 + e + 2)
            } else {
                (text.to_string(), close + 1)
            };
            let (d, t) = self.refs.get(&norm_label(&label))?.clone();
            (d, t, used)
        };
        let title_attr = title.map(|t| format!(" title=\"{}\"", esc(&t))).unwrap_or_default();
        if img {
            let alt = strip_tags(&self.inline(text));
            Some((format!("<img src=\"{}\" alt=\"{}\"{} loading=\"lazy\">", esc(&self.image_src(&dest)), esc(&alt), title_attr), used))
        } else {
            let href = safe_url(&unescape(&dest), false);
            Some((format!("<a href=\"{}\"{}>{}</a>", esc(&href), title_attr, self.inline(text)), used))
        }
    }
}

fn resolve(mut nodes: Vec<Node>) -> String {
    for i in 0..nodes.len() {
        let (ch, close_ok) = match &nodes[i] {
            Node::D { ch, close, .. } => (*ch, *close),
            _ => continue,
        };
        if !close_ok {
            continue;
        }
        while let Node::D { n: n_c, orig: orig_c, open: open_c, .. } = nodes[i] {
            if n_c == 0 {
                break;
            }
            let found = (0..i).rev().find(|&j| match &nodes[j] {
                Node::D { ch: c2, n, orig, open, close, .. } if *c2 == ch && *open && *n > 0 => {
                    let odd = (*close || open_c) && (orig + orig_c) % 3 == 0 && !(orig % 3 == 0 && orig_c % 3 == 0);
                    !odd && (ch != '~' || (*n >= 2 && n_c >= 2))
                }
                _ => false,
            });
            let Some(j) = found else { break };
            let n_o = match &nodes[j] {
                Node::D { n, .. } => *n,
                _ => 0,
            };
            let used = if ch == '~' || (n_o >= 2 && n_c >= 2) { 2 } else { 1 };
            let tag = match (ch, used) {
                ('~', _) => "del",
                (_, 2) => "strong",
                _ => "em",
            };
            if let Node::D { n, pre, .. } = &mut nodes[j] {
                *n -= used;
                *pre = format!("<{}>{}", tag, pre);
            }
            if let Node::D { n, post, .. } = &mut nodes[i] {
                *n -= used;
                post.push_str(&format!("</{}>", tag));
            }
            for n in &mut nodes[j + 1..i] {
                if let Node::D { open, .. } = n {
                    *open = false;
                }
            }
        }
    }
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Raw(s) => out.push_str(&s),
            Node::D { ch, n, pre, post, .. } => {
                out.push_str(&post);
                out.push_str(&ch.to_string().repeat(n));
                out.push_str(&pre);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> String {
        render(s, &Options::default()).html
    }

    #[test]
    fn headings_and_paragraphs() {
        let d = render("# Title *here*\n\nSome **bold** and _it_ and `code <x>`.\nnext line\n\nSetext\n======\n", &Options::default());
        assert!(d.html.contains("<h1 id=\"title-here\" data-line=\"1\"><a class=\"anchor\" href=\"#title-here\">#</a>Title <em>here</em></h1>"));
        assert!(d.html.contains("<p data-line=\"3\">Some <strong>bold</strong> and <em>it</em> and <code>code &lt;x&gt;</code>.\nnext line</p>"));
        assert!(d.html.contains("<h1 id=\"setext\" data-line=\"6\">"));
        assert_eq!(d.title.as_deref(), Some("Title here"));
        assert_eq!(d.headings.len(), 2);
    }

    #[test]
    fn emphasis_edge_cases() {
        assert_eq!(h("snake_case_name and 2*3*4").trim(), "<p data-line=\"1\">snake_case_name and 2<em>3</em>4</p>");
        assert!(h("***both***").contains("<em><strong>both</strong></em>"));
        assert!(h("**a *b* c**").contains("<strong>a <em>b</em> c</strong>"));
        assert!(h("~~gone~~ stays").contains("<del>gone</del> stays"));
        assert!(h("a * b * c").contains("a * b * c"));
    }

    #[test]
    fn links_images_refs() {
        let s = h("[a](http://x.test \"T\") ![alt *x*](p.png) [ref][r] <http://auto.test> http://bare.test/x.\n\n[r]: /there");
        assert!(s.contains("<a href=\"http://x.test\" title=\"T\">a</a>"));
        assert!(s.contains("<img src=\"p.png\" alt=\"alt x\" loading=\"lazy\">"));
        assert!(s.contains("<a href=\"/there\">ref</a>"));
        assert!(s.contains("<a href=\"http://auto.test\">http://auto.test</a>"));
        assert!(s.contains("<a href=\"http://bare.test/x\">http://bare.test/x</a>."));
        assert!(!s.contains("[r]:"));
        assert!(h("[x](javascript:alert(1))").contains("href=\"#\""));
    }

    #[test]
    fn lists() {
        let s = h("- a\n- b\n  - nested\n- [x] done\n- [ ] todo\n\n3. three\n4. four\n");
        assert!(s.contains("<ul data-line=\"1\" class=\"tasks\">"));
        assert!(s.contains("<li data-line=\"1\">a</li>"));
        assert!(s.contains("<li data-line=\"2\">b\n<ul data-line=\"3\">\n<li data-line=\"3\">nested</li>\n</ul></li>"));
        assert!(s.contains("<input type=\"checkbox\" disabled checked> done"));
        assert!(s.contains("<ol data-line=\"7\" start=\"3\">"));
        let loose = h("- a\n\n- b\n");
        assert!(loose.contains("<li data-line=\"1\"><p data-line=\"1\">a</p></li>"));
    }

    #[test]
    fn code_quotes_tables() {
        let s = h("```rust\nfn main() { let x = 1; }\n```\n\n> quote\n> more\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n\n---\n");
        assert!(s.contains("<pre data-line=\"1\" data-lang=\"rust\"><code><span class=\"k1\">fn</span>"));
        assert!(s.contains("<blockquote data-line=\"5\">\n<p data-line=\"5\">quote\nmore</p>\n</blockquote>"));
        assert!(s.contains("<th style=\"text-align:left\">a</th><th style=\"text-align:right\">b</th>"));
        assert!(s.contains("<td style=\"text-align:right\">2</td>"));
        assert!(s.contains("<hr data-line=\"12\">"));
        assert!(h("> [!WARNING]\n> careful").contains("class=\"alert alert-warning\""));
        assert!(h("    indented code\n").contains("<pre data-line=\"1\"><code>indented code</code></pre>"));
    }

    #[test]
    fn html_is_filtered() {
        let s = h("<div align=\"center\">\n<img src=\"a.png\" onerror=\"x()\">\n<script>bad()</script>\n</div>\n\ntext <b>ok</b> <script>no</script> <i onclick=\"y\">z</i>");
        assert!(s.contains("<div align=\"center\">"));
        assert!(!s.contains("<script") && !s.contains("<img") && !s.contains("<i onclick"));
        assert!(s.contains("<b>ok</b>"));
    }

    #[test]
    fn never_panics_on_partial_input() {
        let doc = "# T\n\n- \n-\n- [ ]\n- [x\n  - \n1.\n> \n>\n```\n~~~rs\n|\n| a |\n|-|\n| a | b\n|:-:|\n[x]: \n[a](b \"c\n![\n<div\n<!--\n**_~~`<a href=\n    code\n\t- tab\n***\n===\n";
        // every prefix and every suffix, as while typing, plus every single-line deletion
        for n in 0..=doc.len() {
            if doc.is_char_boundary(n) {
                render(&doc[..n], &Options::default());
                render(&doc[n..], &Options::default());
            }
        }
        let lines: Vec<&str> = doc.lines().collect();
        for k in 0..lines.len() {
            let mut v = lines.clone();
            v.remove(k);
            render(&v.join("\n"), &Options::default());
        }
    }

    #[test]
    fn never_panics_on_random_markdown() {
        let toks = ["- ", "* ", "1. ", "  ", "    ", "\t", "> ", "# ", "## ", "\n", "\n\n", "```", "~~~", "`", "``", "*", "**", "_", "~~", "[", "]", "(", ")", "![", "](", "<", ">", "<div>", "</div>", "<!--", "-->", "|", "|-|", "---", "===", "[ ] ", "[x] ", "\\", "&", "&amp;", "http://a.b/c", "é", "日本", "word", " ", "[x]: u", "<http://x>", "<a@b.c>"];
        let mut seed = 0x9E3779B97F4A7C15u64;
        for _ in 0..4000 {
            let mut doc = String::new();
            for _ in 0..(seed % 40) + 1 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                doc.push_str(toks[(seed % toks.len() as u64) as usize]);
            }
            render(&doc, &Options::default());
        }
    }

    #[test]
    fn unique_ids_and_hard_breaks() {
        let d = render("## Same\n\n## Same\n", &Options::default());
        assert!(d.html.contains("id=\"same\"") && d.html.contains("id=\"same-1\""));
        assert!(render("a\nb", &Options { hard_breaks: true, ..Options::default() }).html.contains("a<br>\nb"));
        assert!(h("a  \nb").contains("a<br>\nb"));
    }
}
