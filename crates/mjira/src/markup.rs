// Lossless converters between Jira's two rich-text formats and one editable
// plain-text (markdown-like) dialect:
//
//     ADF (Cloud)  <-->  text  <-->  wiki markup (Server / Data Center)
//
// Both formats parse into one small AST (`Block` / `Inline`); the text dialect
// parses into the same AST. Anything the dialect cannot express exactly (and
// there is always something) is carried through as a `Raw` node: a fenced
// ```` ```adf-json ```` / ```` ```wiki-raw ```` block, or an inline
// `<adf>…</adf>` / `<wiki>…</wiki>` span. Each block is converted, converted
// back and compared with the original; if the round trip is not exact the
// block is emitted raw instead. So `text_to_adf(adf_to_text(d)) == d` and
// `text_to_wiki(wiki_to_text(w)) == w` always hold, with these deliberate
// equivalences:
//   ADF:  object key order, mark order, adjacent text nodes with the same
//         marks, empty `content`/`attrs`, `order: 1` and `language: ""`.
//   wiki: CRLF vs LF, and leading/trailing whitespace of the whole text.
//
// The text dialect: paragraphs split by blank lines (a single newline or a
// trailing backslash is a line break); `#` headings; `-` / `1.` lists nested by
// indentation; ``` fences; `>` quotes; `---`; GFM tables; `:::kind` panels;
// `**bold**`, `*italic*`, `~~strike~~`, `` `code` ``, `[text](url)`; and
// backslash escapes for anything that would otherwise be read as markup.

use mtui::json::{self, quote, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flavor {
    Adf,
    Wiki,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Marks {
    pub strong: bool,
    pub em: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String, Marks),
    Break,
    Raw(Flavor, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading(usize, Vec<Inline>),
    List { ordered: bool, start: Option<u32>, items: Vec<Vec<Block>> },
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    Rule,
    Table { header: Option<Vec<Vec<Inline>>>, rows: Vec<Vec<Vec<Inline>>> },
    Panel { kind: String, body: Vec<Block> },
    Raw(Flavor, String),
}

fn plain() -> Marks {
    Marks::default()
}

/// Merge neighbouring runs with equal marks and drop empty ones.
fn norm_inl(v: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for i in v {
        match i {
            Inline::Text(t, m) => {
                if t.is_empty() {
                    continue;
                }
                if let Some(Inline::Text(p, pm)) = out.last_mut() {
                    if *pm == m {
                        p.push_str(&t);
                        continue;
                    }
                }
                out.push(Inline::Text(t, m));
            }
            o => out.push(o),
        }
    }
    out
}

/// Move whitespace at the edges of a marked run outside its marks, since
/// `** a**` is not emphasis.
fn hoist_ws(runs: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    for r in runs {
        match r {
            Inline::Text(t, m) if *m != plain() && (t.starts_with(char::is_whitespace) || t.ends_with(char::is_whitespace)) => {
                let core = t.trim();
                if core.is_empty() {
                    out.push(Inline::Text(t.clone(), plain()));
                    continue;
                }
                let lead = &t[..t.len() - t.trim_start().len()];
                let trail = &t[t.trim_end().len()..];
                for (s, mm) in [(lead, plain()), (core, m.clone()), (trail, plain())] {
                    if !s.is_empty() {
                        out.push(Inline::Text(s.to_string(), mm));
                    }
                }
            }
            o => out.push(o.clone()),
        }
    }
    out
}

fn set_link(runs: &mut [Inline], url: &str) {
    for r in runs {
        if let Inline::Text(_, m) = r {
            m.link = Some(url.to_string());
        }
    }
}

// ---- generic span emitter (marks -> nested delimiters) ----

#[derive(Clone, PartialEq)]
enum Open {
    Link(String),
    Strike,
    Strong,
    Em,
}

struct Style<'a> {
    want: &'a dyn Fn(&str, &Marks) -> Vec<Open>,
    open: &'a dyn Fn(&Open) -> String,
    close: &'a dyn Fn(&Open) -> String,
    /// text, marks, at the very start of a line
    leaf: &'a dyn Fn(&str, &Marks, bool) -> String,
    raw: &'a dyn Fn(Flavor, &str) -> String,
    brk: &'a str,
}

fn emit_spans(runs: &[Inline], fresh0: bool, st: &Style) -> String {
    let mut out = String::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut fresh = fresh0;
    let close_all = |stack: &mut Vec<Open>, out: &mut String| {
        while let Some(o) = stack.pop() {
            out.push_str(&(st.close)(&o));
        }
    };
    for r in runs {
        match r {
            Inline::Text(t, m) => {
                let want = (st.want)(t, m);
                let k = stack.iter().zip(&want).take_while(|(a, b)| a == b).count();
                while stack.len() > k {
                    let o = stack.pop().unwrap();
                    out.push_str(&(st.close)(&o));
                }
                for o in &want[k..] {
                    out.push_str(&(st.open)(o));
                    stack.push(o.clone());
                }
                out.push_str(&(st.leaf)(t, m, fresh && stack.is_empty()));
                fresh = false;
            }
            Inline::Break => {
                close_all(&mut stack, &mut out);
                out.push_str(st.brk);
                fresh = true;
            }
            Inline::Raw(f, s) => {
                close_all(&mut stack, &mut out);
                out.push_str(&(st.raw)(*f, s));
                fresh = false;
            }
        }
    }
    close_all(&mut stack, &mut out);
    out
}

// ---- text dialect: inline emit ----

fn at(c: &[char], i: usize, s: &str) -> bool {
    s.chars().enumerate().all(|(k, ch)| c.get(i + k) == Some(&ch))
}

fn esc(t: &str, in_link: bool, cell: bool) -> String {
    let c: Vec<char> = t.chars().collect();
    let mut o = String::with_capacity(t.len());
    for (i, &ch) in c.iter().enumerate() {
        let hit = match ch {
            '\\' | '*' | '`' | '[' => true,
            ']' => in_link,
            '|' => cell,
            '~' => c.get(i + 1) == Some(&'~'),
            '<' => at(&c, i, "<adf>") || at(&c, i, "<wiki>") || at(&c, i, "<!--"),
            _ => false,
        };
        if hit {
            o.push('\\');
        }
        o.push(ch);
    }
    o
}

fn esc_url(u: &str) -> String {
    let mut o = String::new();
    for ch in u.chars() {
        if matches!(ch, '\\' | ')' | ' ') {
            o.push('\\');
        }
        o.push(ch);
    }
    o
}

fn esc_payload(s: &str) -> String {
    s.replace('\\', "\\\\").replace('<', "\\<")
}

/// Escape what would turn a paragraph line into some other block.
fn line_start_esc(s: String, cell: bool) -> String {
    if s.starts_with(' ') {
        return format!("\\{}", s);
    }
    if cell {
        return s;
    }
    let b = s.as_bytes();
    let Some(&f) = b.first() else { return s };
    let eol_or_sp = |i: usize| b.len() == i || b[i] == b' ';
    match f {
        b'#' => {
            let n = b.iter().take_while(|&&x| x == b'#').count();
            if n <= 6 && eol_or_sp(n) {
                return format!("\\{}", s);
            }
        }
        b'-' | b'+' if eol_or_sp(1) || s.starts_with("---") => return format!("\\{}", s),
        b'>' | b'|' => return format!("\\{}", s),
        b':' if s.starts_with(":::") => return format!("\\{}", s),
        b'0'..=b'9' => {
            let d = b.iter().take_while(|x| x.is_ascii_digit()).count();
            if d <= 9 && d < b.len() && matches!(b[d], b'.' | b')') && eol_or_sp(d + 1) {
                return format!("{}\\{}", &s[..d], &s[d..]);
            }
        }
        _ => {}
    }
    s
}

fn code_span(t: &str) -> String {
    let mut run = 0;
    let mut best = 0;
    for ch in t.chars() {
        if ch == '`' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(best + 1);
    let pad = if t.starts_with([' ', '`']) || t.ends_with([' ', '`']) { " " } else { "" };
    format!("{}{}{}{}{}", fence, pad, t, pad, fence)
}

fn text_inline(runs: &[Inline], fresh0: bool, cell: bool) -> String {
    let runs = hoist_ws(&norm_inl(runs.to_vec()));
    let want = |_: &str, m: &Marks| {
        let mut v = Vec::new();
        if let Some(u) = &m.link {
            v.push(Open::Link(u.clone()));
        }
        if m.strike {
            v.push(Open::Strike);
        }
        if m.strong {
            v.push(Open::Strong);
        }
        if m.em {
            v.push(Open::Em);
        }
        v
    };
    let open = |o: &Open| match o {
        Open::Link(_) => "[".to_string(),
        Open::Strike => "~~".into(),
        Open::Strong => "**".into(),
        Open::Em => "*".into(),
    };
    let close = |o: &Open| match o {
        Open::Link(u) => format!("]({})", esc_url(u)),
        Open::Strike => "~~".into(),
        Open::Strong => "**".into(),
        Open::Em => "*".into(),
    };
    let leaf = |t: &str, m: &Marks, fresh: bool| {
        if m.code {
            return code_span(t);
        }
        let s = esc(t, m.link.is_some(), cell);
        if fresh {
            line_start_esc(s, cell)
        } else {
            s
        }
    };
    let raw = |f: Flavor, s: &str| {
        let tag = if f == Flavor::Adf { "adf" } else { "wiki" };
        format!("<{}>{}</{}>", tag, esc_payload(s), tag)
    };
    emit_spans(&runs, fresh0, &Style { want: &want, open: &open, close: &close, leaf: &leaf, raw: &raw, brk: "\\\n" })
}

// ---- text dialect: inline parse ----

struct P {
    c: Vec<char>,
    i: usize,
}

impl P {
    fn starts(&self, s: &str) -> bool {
        at(&self.c, self.i, s)
    }

    /// Parse until `closer` (consumed). None when it never comes.
    fn run(&mut self, closer: Option<&str>, m: &Marks) -> Option<Vec<Inline>> {
        let mut out: Vec<Inline> = Vec::new();
        let mut buf = String::new();
        macro_rules! flush {
            () => {
                if !buf.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut buf), m.clone()));
                }
            };
        }
        let start = self.i;
        while self.i < self.c.len() {
            if let Some(cl) = closer {
                if self.starts(cl) && self.i > start && !self.c[self.i - 1].is_whitespace() || (cl == "]" && self.starts(cl)) {
                    self.i += cl.chars().count();
                    flush!();
                    return Some(out);
                }
            }
            let ch = self.c[self.i];
            match ch {
                '\\' => match self.c.get(self.i + 1) {
                    Some('\n') => {
                        flush!();
                        out.push(Inline::Break);
                        self.i += 2;
                    }
                    Some(&n) if n.is_ascii_punctuation() || n == ' ' => {
                        buf.push(n);
                        self.i += 2;
                    }
                    _ => {
                        buf.push('\\');
                        self.i += 1;
                    }
                },
                '\n' => {
                    flush!();
                    out.push(Inline::Break);
                    self.i += 1;
                }
                '`' => {
                    let n = self.c[self.i..].iter().take_while(|&&x| x == '`').count();
                    let mut j = self.i + n;
                    let mut found = None;
                    while j < self.c.len() {
                        if self.c[j] == '`' {
                            let r = self.c[j..].iter().take_while(|&&x| x == '`').count();
                            if r == n {
                                found = Some(j);
                                break;
                            }
                            j += r;
                        } else {
                            j += 1;
                        }
                    }
                    match found {
                        Some(j) => {
                            let mut body: String = self.c[self.i + n..j].iter().collect();
                            if body.len() >= 2 && body.starts_with(' ') && body.ends_with(' ') && body.trim() != "" {
                                body = body[1..body.len() - 1].to_string();
                            }
                            flush!();
                            out.push(Inline::Text(body, Marks { code: true, ..m.clone() }));
                            self.i = j + n;
                        }
                        None => {
                            buf.push_str(&"`".repeat(n));
                            self.i += n;
                        }
                    }
                }
                '<' if self.starts("<adf>") || self.starts("<wiki>") => {
                    let (tag, f) = if self.starts("<adf>") { ("adf", Flavor::Adf) } else { ("wiki", Flavor::Wiki) };
                    let close = format!("</{}>", tag);
                    let mut j = self.i + tag.len() + 2;
                    let mut payload = String::new();
                    let mut ok = false;
                    while j < self.c.len() {
                        if at(&self.c, j, &close) {
                            ok = true;
                            break;
                        }
                        if self.c[j] == '\\' && j + 1 < self.c.len() {
                            payload.push(self.c[j + 1]);
                            j += 2;
                        } else {
                            payload.push(self.c[j]);
                            j += 1;
                        }
                    }
                    if ok {
                        flush!();
                        out.push(Inline::Raw(f, payload));
                        self.i = j + close.len();
                    } else {
                        buf.push('<');
                        self.i += 1;
                    }
                }
                '[' => {
                    let save = self.i;
                    self.i += 1;
                    let mut done = false;
                    if let Some(mut inner) = self.run(Some("]"), m) {
                        if self.c.get(self.i) == Some(&'(') {
                            let mut j = self.i + 1;
                            let mut url = String::new();
                            while j < self.c.len() && self.c[j] != ')' {
                                if self.c[j] == '\\' && j + 1 < self.c.len() {
                                    j += 1;
                                }
                                url.push(self.c[j]);
                                j += 1;
                            }
                            if j < self.c.len() {
                                set_link(&mut inner, &url);
                                flush!();
                                out.extend(inner);
                                self.i = j + 1;
                                done = true;
                            }
                        }
                    }
                    if !done {
                        self.i = save + 1;
                        buf.push('[');
                    }
                }
                '~' | '*' => {
                    let (dl, m2) = if self.starts("~~") {
                        (2, Marks { strike: true, ..m.clone() })
                    } else if self.starts("**") {
                        (2, Marks { strong: true, ..m.clone() })
                    } else if ch == '*' {
                        (1, Marks { em: true, ..m.clone() })
                    } else {
                        buf.push('~');
                        self.i += 1;
                        continue;
                    };
                    let delim: String = self.c[self.i..self.i + dl].iter().collect();
                    let save = self.i;
                    let flanked = self.c.get(self.i + dl).is_some_and(|x| !x.is_whitespace());
                    self.i += dl;
                    let inner = if flanked { self.run(Some(&delim), &m2) } else { None };
                    match inner {
                        Some(v) if !v.is_empty() => {
                            flush!();
                            out.extend(v);
                        }
                        _ => {
                            self.i = save + 1;
                            buf.push(ch);
                        }
                    }
                }
                _ => {
                    buf.push(ch);
                    self.i += 1;
                }
            }
        }
        if closer.is_some() {
            return None;
        }
        flush!();
        Some(out)
    }
}

fn parse_inline(s: &str) -> Vec<Inline> {
    let mut p = P { c: s.chars().collect(), i: 0 };
    norm_inl(p.run(None, &plain()).unwrap_or_default())
}

// ---- text dialect: blocks emit ----

fn fence_for(s: &str) -> String {
    let mut run = 0;
    let mut best = 0;
    for ch in s.chars() {
        if ch == '`' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((best + 1).max(3))
}

fn indent(t: &str, first: &str, pad: &str) -> String {
    t.split('\n').enumerate().map(|(k, l)| if k == 0 { format!("{}{}", first, l) } else if l.is_empty() { String::new() } else { format!("{}{}", pad, l) }).collect::<Vec<_>>().join("\n")
}

fn join_blocks(bs: &[Block], tight: bool) -> String {
    let mut out = String::new();
    for (i, b) in bs.iter().enumerate() {
        if i > 0 {
            match (&bs[i - 1], b) {
                (Block::List { ordered: a, .. }, Block::List { ordered: c, .. }) if a == c => out.push_str("\n\n<!---->\n\n"),
                (Block::Para(_), Block::List { .. }) if tight => out.push('\n'),
                _ => out.push_str("\n\n"),
            }
        }
        out.push_str(&emit_block(b));
    }
    out
}

fn emit_block(b: &Block) -> String {
    match b {
        Block::Para(i) => text_inline(i, true, false),
        Block::Heading(n, i) => format!("{} {}", "#".repeat(*n), text_inline(i, false, false)),
        Block::List { ordered, start, items } => {
            let mut lines = Vec::new();
            for (k, it) in items.iter().enumerate() {
                let marker = if *ordered { format!("{}. ", start.unwrap_or(1) as usize + k) } else { "- ".to_string() };
                let body = join_blocks(it, true);
                lines.push(indent(&body, &marker, &" ".repeat(marker.len())));
            }
            lines.join("\n")
        }
        Block::Code { lang, text } => {
            let f = fence_for(text);
            if text.is_empty() {
                format!("{}{}\n{}", f, lang, f)
            } else {
                format!("{}{}\n{}\n{}", f, lang, text, f)
            }
        }
        Block::Quote(bs) => {
            let t = join_blocks(bs, false);
            t.split('\n').map(|l| if l.is_empty() { ">".to_string() } else { format!("> {}", l) }).collect::<Vec<_>>().join("\n")
        }
        Block::Rule => "---".into(),
        Block::Table { header, rows } => {
            let cols = header.as_ref().map(|h| h.len()).or_else(|| rows.first().map(|r| r.len())).unwrap_or(0);
            let row = |cells: &[Vec<Inline>]| format!("|{}|", cells.iter().map(|c| format!(" {} ", fix_cell(text_inline(c, true, true)))).collect::<Vec<_>>().join("|"));
            let mut out = Vec::new();
            out.push(match header {
                Some(h) => row(h),
                None => row(&vec![Vec::new(); cols]),
            });
            out.push(format!("|{}", " --- |".repeat(cols)));
            for r in rows {
                out.push(row(r));
            }
            out.join("\n")
        }
        Block::Panel { kind, body } => format!(":::{}\n{}\n:::", kind, join_blocks(body, false)),
        Block::Raw(f, s) => {
            let fence = fence_for(s);
            format!("{}{}\n{}\n{}", fence, if *f == Flavor::Adf { "adf-json" } else { "wiki-raw" }, s, fence)
        }
    }
}

/// A trailing space would be trimmed when the cell is read back.
fn fix_cell(s: String) -> String {
    if s.ends_with(' ') {
        format!("{}\\ ", &s[..s.len() - 1])
    } else {
        s
    }
}

// ---- text dialect: blocks parse ----

fn fence_len(l: &str) -> usize {
    let n = l.chars().take_while(|&c| c == '`').count();
    if n >= 3 {
        n
    } else {
        0
    }
}

fn list_marker(l: &str) -> Option<(bool, u32, usize)> {
    let b = l.as_bytes();
    if matches!(b.first(), Some(b'-' | b'*' | b'+')) && (b.len() == 1 || b[1] == b' ') {
        return Some((false, 0, if b.len() == 1 { 1 } else { 2 }));
    }
    let d = b.iter().take_while(|x| x.is_ascii_digit()).count();
    if (1..=9).contains(&d) && b.len() > d && matches!(b[d], b'.' | b')') && (b.len() == d + 1 || b[d + 1] == b' ') {
        return Some((true, l[..d].parse().unwrap_or(1), (d + 2).min(b.len())));
    }
    None
}

fn heading_of(l: &str) -> Option<(usize, &str)> {
    let n = l.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &l[n..];
    if rest.is_empty() {
        Some((n, ""))
    } else {
        rest.strip_prefix(' ').map(|r| (n, r))
    }
}

fn is_delim_row(l: &str) -> bool {
    l.starts_with('|') && {
        let cells = split_cells(l);
        !cells.is_empty() && cells.iter().all(|c| {
            let t = c.trim();
            let t = t.strip_prefix(':').unwrap_or(t);
            let t = t.strip_suffix(':').unwrap_or(t);
            !t.is_empty() && t.chars().all(|x| x == '-')
        })
    }
}

fn split_cells(l: &str) -> Vec<String> {
    let c: Vec<char> = l.chars().collect();
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut i = if c.first() == Some(&'|') { 1 } else { 0 };
    let mut closed = false;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() {
            cur.push(c[i]);
            cur.push(c[i + 1]);
            i += 2;
            continue;
        }
        if c[i] == '|' {
            cells.push(std::mem::take(&mut cur));
            closed = true;
        } else {
            cur.push(c[i]);
            closed = false;
        }
        i += 1;
    }
    if !closed && !cur.is_empty() {
        cells.push(cur);
    }
    cells
}

fn trim_cell(s: &str) -> String {
    let t = s.trim_start_matches(' ');
    let e = t.trim_end_matches(' ');
    let removed = t.len() - e.len();
    let bs = e.chars().rev().take_while(|&c| c == '\\').count();
    if removed > 0 && bs % 2 == 1 {
        t[..e.len() + 1].to_string()
    } else {
        e.to_string()
    }
}

fn starts_block(lines: &[String], i: usize) -> bool {
    let l = &lines[i];
    fence_len(l) > 0
        || heading_of(l).is_some()
        || l == "---"
        || l == "<!---->"
        || (l.starts_with(":::") && l.trim() != ":::")
        || l.starts_with('>')
        || list_marker(l).is_some()
        || (l.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_delim_row(n)))
}

pub fn parse_blocks(text: &str) -> Vec<Block> {
    let lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
    blocks_from(&lines)
}

fn blocks_from(lines: &[String]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if l.trim().is_empty() || l == "<!---->" {
            i += 1;
            continue;
        }
        let n = fence_len(l);
        if n > 0 {
            let info = l[n..].trim().to_string();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_end();
                if t.len() >= n && t.chars().all(|c| c == '`') {
                    break;
                }
                body.push(lines[i].as_str());
                i += 1;
            }
            i += 1;
            let text = body.join("\n");
            out.push(match info.as_str() {
                "adf-json" => Block::Raw(Flavor::Adf, text),
                "wiki-raw" => Block::Raw(Flavor::Wiki, text),
                _ => Block::Code { lang: info, text },
            });
            continue;
        }
        if let Some((n, rest)) = heading_of(l) {
            out.push(Block::Heading(n, parse_inline(rest)));
            i += 1;
            continue;
        }
        if l == "---" {
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if l.starts_with(":::") && l.trim() != ":::" {
            let kind = l[3..].trim().to_string();
            let mut depth = 1;
            let mut fence = 0;
            let mut j = i + 1;
            while j < lines.len() {
                let t = &lines[j];
                if fence > 0 {
                    let tt = t.trim_end();
                    if tt.len() >= fence && tt.chars().all(|c| c == '`') {
                        fence = 0;
                    }
                } else if fence_len(t) > 0 {
                    fence = fence_len(t);
                } else if t.starts_with(":::") {
                    if t.trim() == ":::" {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        depth += 1;
                    }
                }
                j += 1;
            }
            out.push(Block::Panel { kind, body: blocks_from(&lines[i + 1..j.min(lines.len())]) });
            i = j + 1;
            continue;
        }
        if l.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].starts_with('>') {
                let r = &lines[i][1..];
                inner.push(r.strip_prefix(' ').unwrap_or(r).to_string());
                i += 1;
            }
            out.push(Block::Quote(blocks_from(&inner)));
            continue;
        }
        if let Some((ordered, num, _)) = list_marker(l) {
            let start = (ordered && num != 1).then_some(num);
            let mut items = Vec::new();
            loop {
                let Some((o, _, off)) = list_marker(&lines[i]) else { break };
                if o != ordered {
                    break;
                }
                let mut body = vec![lines[i][off..].to_string()];
                i += 1;
                while i < lines.len() {
                    let l = &lines[i];
                    if l.trim().is_empty() {
                        let mut k = i;
                        while k < lines.len() && lines[k].trim().is_empty() {
                            k += 1;
                        }
                        if k < lines.len() && lines[k].starts_with(' ') {
                            body.extend((i..k).map(|_| String::new()));
                            i = k;
                            continue;
                        }
                        break;
                    } else if l.starts_with(' ') {
                        let n = l.chars().take_while(|&c| c == ' ').count().min(off);
                        body.push(l[n..].to_string());
                        i += 1;
                    } else {
                        break;
                    }
                }
                items.push(blocks_from(&body));
                let mut k = i;
                while k < lines.len() && lines[k].trim().is_empty() {
                    k += 1;
                }
                match lines.get(k).and_then(|l| list_marker(l)) {
                    Some((o, _, _)) if o == ordered => i = k,
                    _ => break,
                }
            }
            out.push(Block::List { ordered, start, items });
            continue;
        }
        if l.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_delim_row(n)) {
            let cells = |l: &str| split_cells(l).iter().map(|c| parse_inline(&trim_cell(c))).collect::<Vec<_>>();
            let h = cells(l);
            let header = if h.iter().all(|c| c.is_empty()) { None } else { Some(h) };
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].starts_with('|') {
                rows.push(cells(&lines[i]));
                i += 1;
            }
            out.push(Block::Table { header, rows });
            continue;
        }
        let mut para = vec![l.as_str()];
        i += 1;
        while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(lines, i) {
            para.push(lines[i].as_str());
            i += 1;
        }
        out.push(Block::Para(parse_inline(&para.join("\n"))));
    }
    out
}

// ---- JSON helpers ----

pub fn to_json(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", *n as i64)
            } else {
                format!("{}", n)
            }
        }
        Value::Str(s) => quote(s),
        Value::Arr(a) => format!("[{}]", a.iter().map(to_json).collect::<Vec<_>>().join(",")),
        Value::Obj(o) => format!("{{{}}}", o.iter().map(|(k, x)| format!("{}:{}", quote(k), to_json(x))).collect::<Vec<_>>().join(",")),
    }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn s(x: &str) -> Value {
    Value::Str(x.to_string())
}

/// Canonical form for comparing two ADF values (see the header comment).
pub fn norm(v: &Value) -> Value {
    match v {
        Value::Obj(o) => {
            let mut o: Vec<(String, Value)> = o.iter().map(|(k, x)| (k.clone(), norm(x))).collect();
            for (k, x) in o.iter_mut() {
                match (k.as_str(), &mut *x) {
                    ("attrs", Value::Obj(a)) => a.retain(|(n, y)| !(n == "order" && *y == Value::Num(1.0)) && !(n == "language" && *y == Value::Str(String::new()))),
                    ("marks", Value::Arr(a)) => a.sort_by_key(to_json),
                    _ => {}
                }
            }
            o.retain(|(k, x)| !(matches!(k.as_str(), "attrs") && matches!(x, Value::Obj(a) if a.is_empty())) && !(matches!(k.as_str(), "content" | "marks") && matches!(x, Value::Arr(a) if a.is_empty())));
            o.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Obj(o)
        }
        Value::Arr(a) => {
            let mut out: Vec<Value> = Vec::new();
            for x in a.iter().map(norm) {
                if x.get("type").str() == "text" {
                    if x.get("text").str().is_empty() {
                        continue;
                    }
                    if let Some(p) = out.last_mut() {
                        if p.get("type").str() == "text" && p.get("marks") == x.get("marks") {
                            if let Value::Obj(po) = p {
                                for (k, pv) in po.iter_mut() {
                                    if k == "text" {
                                        *pv = Value::Str(format!("{}{}", pv.str(), x.get("text").str()));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
                out.push(x);
            }
            Value::Arr(out)
        }
        o => o.clone(),
    }
}

// ---- ADF -> AST ----

fn keys_within(n: &Value, ok: &[&str]) -> bool {
    match n {
        Value::Obj(o) => o.iter().all(|(k, _)| ok.contains(&k.as_str())),
        _ => false,
    }
}

fn raw_adf(n: &Value) -> Block {
    Block::Raw(Flavor::Adf, to_json(n))
}

fn inlines_from(nodes: &[Value]) -> Vec<Inline> {
    let mut out = Vec::new();
    for n in nodes {
        let raw = || Inline::Raw(Flavor::Adf, to_json(n));
        match n.get("type").str() {
            "text" if keys_within(n, &["type", "text", "marks"]) => {
                let mut m = plain();
                let mut ok = true;
                for mk in n.get("marks").arr() {
                    let attrs = mk.get("attrs");
                    let bare = attrs.is_null() || matches!(attrs, Value::Obj(a) if a.is_empty());
                    match mk.get("type").str() {
                        "strong" if bare => m.strong = true,
                        "em" if bare => m.em = true,
                        "strike" if bare => m.strike = true,
                        "code" if bare => m.code = true,
                        "link" if matches!(attrs, Value::Obj(a) if a.len() == 1 && a[0].0 == "href") && attrs.get("href").opt_str().is_some() => m.link = Some(attrs.get("href").str().to_string()),
                        _ => ok = false,
                    }
                }
                if ok {
                    out.push(Inline::Text(n.get("text").str().to_string(), m));
                } else {
                    out.push(raw());
                }
            }
            "hardBreak" if keys_within(n, &["type"]) => out.push(Inline::Break),
            _ => out.push(raw()),
        }
    }
    norm_inl(out)
}

fn blocks_of(nodes: &[Value]) -> Vec<Block> {
    nodes.iter().map(block_from).collect()
}

fn block_from(n: &Value) -> Block {
    if !keys_within(n, &["type", "content", "attrs"]) {
        return raw_adf(n);
    }
    let kids = n.get("content").arr();
    match n.get("type").str() {
        "paragraph" => Block::Para(inlines_from(kids)),
        "heading" => {
            let l = n.path("attrs.level").num() as usize;
            if (1..=6).contains(&l) {
                Block::Heading(l, inlines_from(kids))
            } else {
                raw_adf(n)
            }
        }
        t @ ("bulletList" | "orderedList") => {
            if kids.iter().any(|k| k.get("type").str() != "listItem" || !keys_within(k, &["type", "content"])) {
                return raw_adf(n);
            }
            let start = (!n.path("attrs.order").is_null()).then(|| n.path("attrs.order").num() as u32).filter(|&o| o != 1);
            Block::List { ordered: t == "orderedList", start, items: kids.iter().map(|k| blocks_of(k.get("content").arr())).collect() }
        }
        "codeBlock" => {
            if kids.iter().any(|k| k.get("type").str() != "text" || !keys_within(k, &["type", "text"])) {
                return raw_adf(n);
            }
            Block::Code { lang: n.path("attrs.language").str().to_string(), text: kids.iter().map(|k| k.get("text").str()).collect() }
        }
        "blockquote" => Block::Quote(blocks_of(kids)),
        "rule" => Block::Rule,
        "panel" => Block::Panel { kind: n.path("attrs.panelType").str().to_string(), body: blocks_of(kids) },
        "table" => table_from(n).unwrap_or_else(|| raw_adf(n)),
        _ => raw_adf(n),
    }
}

fn table_from(n: &Value) -> Option<Block> {
    let mut rows: Vec<(bool, Vec<Vec<Inline>>)> = Vec::new();
    for r in n.get("content").arr() {
        if r.get("type").str() != "tableRow" || !keys_within(r, &["type", "content"]) {
            return None;
        }
        let mut cells = Vec::new();
        let mut all_head = true;
        for c in r.get("content").arr() {
            let t = c.get("type").str();
            if !matches!(t, "tableHeader" | "tableCell") || !keys_within(c, &["type", "content"]) {
                return None;
            }
            all_head &= t == "tableHeader";
            let ps = c.get("content").arr();
            if ps.len() != 1 || ps[0].get("type").str() != "paragraph" || !keys_within(&ps[0], &["type", "content"]) {
                return None;
            }
            let inl = inlines_from(ps[0].get("content").arr());
            if inl.iter().any(|i| matches!(i, Inline::Break)) {
                return None;
            }
            cells.push((t == "tableHeader", inl));
        }
        if cells.iter().any(|c| c.0) && !all_head {
            return None;
        }
        rows.push((all_head && !cells.is_empty(), cells.into_iter().map(|c| c.1).collect()));
    }
    let heads = rows.iter().filter(|r| r.0).count();
    if rows.is_empty() || heads > 1 || (heads == 1 && !rows[0].0) {
        return None;
    }
    let header = rows[0].0.then(|| rows[0].1.clone());
    let body = rows.into_iter().skip(header.is_some() as usize).map(|r| r.1).collect();
    Some(Block::Table { header, rows: body })
}

// ---- AST -> ADF ----

fn marks_json(m: &Marks) -> Value {
    let mut v = Vec::new();
    let t = |n: &str| obj(vec![("type", s(n))]);
    if m.strong {
        v.push(t("strong"));
    }
    if m.em {
        v.push(t("em"));
    }
    if m.strike {
        v.push(t("strike"));
    }
    if m.code {
        v.push(t("code"));
    }
    if let Some(u) = &m.link {
        v.push(obj(vec![("type", s("link")), ("attrs", obj(vec![("href", s(u))]))]));
    }
    Value::Arr(v)
}

fn text_node(t: &str, m: &Marks) -> Value {
    let mut p = vec![("type", s("text")), ("text", s(t))];
    if *m != plain() {
        p.push(("marks", marks_json(m)));
    }
    obj(p)
}

fn inlines_json(v: &[Inline]) -> Value {
    Value::Arr(
        v.iter()
            .map(|i| match i {
                Inline::Text(t, m) => text_node(t, m),
                Inline::Break => obj(vec![("type", s("hardBreak"))]),
                Inline::Raw(Flavor::Adf, r) => json::parse(r).unwrap_or_else(|_| text_node(r, &plain())),
                Inline::Raw(_, r) => text_node(r, &plain()),
            })
            .collect(),
    )
}

fn node(t: &str, attrs: Option<Value>, content: Value) -> Value {
    let mut p = vec![("type", s(t))];
    if let Some(a) = attrs {
        p.push(("attrs", a));
    }
    if !matches!(&content, Value::Arr(a) if a.is_empty()) {
        p.push(("content", content));
    }
    obj(p)
}

fn blocks_json(bs: &[Block]) -> Value {
    Value::Arr(bs.iter().map(block_json).collect())
}

fn block_json(b: &Block) -> Value {
    match b {
        Block::Para(i) => node("paragraph", None, inlines_json(i)),
        Block::Heading(n, i) => node("heading", Some(obj(vec![("level", Value::Num(*n as f64))])), inlines_json(i)),
        Block::List { ordered, start, items } => {
            let its = Value::Arr(items.iter().map(|it| node("listItem", None, blocks_json(it))).collect());
            let attrs = start.map(|o| obj(vec![("order", Value::Num(o as f64))]));
            node(if *ordered { "orderedList" } else { "bulletList" }, attrs.filter(|_| *ordered), its)
        }
        Block::Code { lang, text } => {
            let attrs = (!lang.is_empty()).then(|| obj(vec![("language", s(lang))]));
            let content = if text.is_empty() { Value::Arr(vec![]) } else { Value::Arr(vec![text_node(text, &plain())]) };
            node("codeBlock", attrs, content)
        }
        Block::Quote(bs) => node("blockquote", None, blocks_json(bs)),
        Block::Rule => node("rule", None, Value::Arr(vec![])),
        Block::Table { header, rows } => {
            let cell = |kind: &str, c: &[Inline]| node(kind, None, Value::Arr(vec![node("paragraph", None, inlines_json(c))]));
            let mut trs = Vec::new();
            if let Some(h) = header {
                trs.push(node("tableRow", None, Value::Arr(h.iter().map(|c| cell("tableHeader", c)).collect())));
            }
            for r in rows {
                trs.push(node("tableRow", None, Value::Arr(r.iter().map(|c| cell("tableCell", c)).collect())));
            }
            node("table", None, Value::Arr(trs))
        }
        Block::Panel { kind, body } => node("panel", Some(obj(vec![("panelType", s(kind))])), blocks_json(body)),
        Block::Raw(Flavor::Adf, r) => json::parse(r).unwrap_or_else(|_| node("codeBlock", None, Value::Arr(vec![text_node(r, &plain())]))),
        Block::Raw(_, r) => node("codeBlock", None, Value::Arr(vec![text_node(r, &plain())])),
    }
}

// ---- public: ADF <-> text ----

fn adf_doc(content: Value) -> Value {
    obj(vec![("version", Value::Num(1.0)), ("type", s("doc")), ("content", content)])
}

/// An ADF document as editable text.
pub fn adf_to_text(doc: &Value) -> String {
    let nodes = doc.get("content").arr();
    let mut blocks = Vec::new();
    for n in nodes {
        let b = block_from(n);
        let t = emit_block(&b);
        let back = parse_blocks(&t);
        let ok = back.len() == 1 && norm(&block_json(&back[0])) == norm(n);
        blocks.push(if ok { b } else { raw_adf(n) });
    }
    let text = join_blocks(&blocks, false);
    // adjacency can still change the meaning; fall back to all-raw if so
    if norm(&text_to_adf(&text)) != norm(&adf_doc(Value::Arr(nodes.to_vec()))) {
        let raws: Vec<Block> = nodes.iter().map(raw_adf).collect();
        return join_blocks(&raws, false);
    }
    text
}

/// Editable text back to an ADF document.
pub fn text_to_adf(text: &str) -> Value {
    adf_doc(blocks_json(&parse_blocks(text)))
}

// ---- wiki markup: inline ----

const WIKI_SPECIAL: &str = "*_-+^~?{}[]!|#";

/// Split on `sep` outside `[...]`, `{{...}}` and backslash escapes.
fn split_wiki_cells(s: &str, sep: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    let mut depth = 0;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() {
            cur.push(c[i]);
            cur.push(c[i + 1]);
            i += 2;
            continue;
        }
        if at(&c, i, "{{") {
            if let Some(e) = (i + 2..c.len()).find(|&j| at(&c, j, "}}")) {
                cur.extend(&c[i..e + 2]);
                i = e + 2;
                continue;
            }
        }
        match c[i] {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ => {}
        }
        if depth == 0 && at(&c, i, sep) {
            cells.push(std::mem::take(&mut cur));
            i += sep.len();
            continue;
        }
        cur.push(c[i]);
        i += 1;
    }
    cells.push(cur);
    cells
}

fn wiki_inline(src: &str, m: &Marks) -> Vec<Inline> {
    let c: Vec<char> = src.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut buf), m.clone()));
            }
        };
    }
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' && c.get(i + 1).is_some_and(|n| WIKI_SPECIAL.contains(*n)) {
            buf.push(c[i + 1]);
            i += 2;
            continue;
        }
        if at(&c, i, "{{") {
            if let Some(e) = (i + 3..c.len()).find(|&j| at(&c, j, "}}")) {
                flush!();
                out.push(Inline::Text(c[i + 2..e].iter().collect(), Marks { code: true, ..m.clone() }));
                i = e + 2;
                continue;
            }
        }
        if ch == '[' {
            if let Some(e) = (i + 1..c.len()).find(|&j| c[j] == ']') {
                let inner = &c[i + 1..e];
                let pipe = inner.iter().position(|&x| x == '|');
                let (text, url): (Option<String>, String) = match pipe {
                    Some(p) if p > 0 && p + 1 < inner.len() => (Some(inner[..p].iter().collect()), inner[p + 1..].iter().collect()),
                    None if inner.starts_with(&['h', 't', 't', 'p']) => (None, inner.iter().collect()),
                    _ => (Some(String::new()), String::new()),
                };
                if !url.is_empty() {
                    let mut runs = match &text {
                        Some(t) => wiki_inline(t, m),
                        None => vec![Inline::Text(url.clone(), m.clone())],
                    };
                    set_link(&mut runs, &url);
                    flush!();
                    out.extend(runs);
                    i = e + 1;
                    continue;
                }
            }
        }
        if matches!(ch, '*' | '_' | '-') && (i == 0 || !c[i - 1].is_alphanumeric()) {
            if let Some(e) = (i + 1..c.len()).find(|&j| c[j] == ch && c[j - 1] != '\\') {
                let inner = &c[i + 1..e];
                let after = c.get(e + 1);
                if !inner.is_empty() && !inner[0].is_whitespace() && !inner[inner.len() - 1].is_whitespace() && after.is_none_or(|a| !a.is_alphanumeric()) {
                    let mut m2 = m.clone();
                    match ch {
                        '*' => m2.strong = true,
                        '_' => m2.em = true,
                        _ => m2.strike = true,
                    }
                    let t: String = inner.iter().collect();
                    flush!();
                    out.extend(wiki_inline(&t, &m2));
                    i = e + 1;
                    continue;
                }
            }
        }
        buf.push(ch);
        i += 1;
    }
    flush!();
    out
}

fn wiki_inlines(src: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for (k, l) in src.split('\n').enumerate() {
        if k > 0 {
            out.push(Inline::Break);
        }
        out.extend(wiki_inline(l, &plain()));
    }
    norm_inl(out)
}

fn wiki_inline_emit(runs: &[Inline], escape: bool) -> String {
    let want = |t: &str, m: &Marks| {
        let mut v = Vec::new();
        if m.link.as_deref() == Some(t) && !m.strong && !m.em && !m.strike && !m.code {
            return v;
        }
        if m.strong {
            v.push(Open::Strong);
        }
        if m.em {
            v.push(Open::Em);
        }
        if m.strike {
            v.push(Open::Strike);
        }
        if let Some(u) = &m.link {
            v.push(Open::Link(u.clone()));
        }
        v
    };
    let open = |o: &Open| match o {
        Open::Strong => "*".to_string(),
        Open::Em => "_".into(),
        Open::Strike => "-".into(),
        Open::Link(_) => "[".into(),
    };
    let close = |o: &Open| match o {
        Open::Strong => "*".to_string(),
        Open::Em => "_".into(),
        Open::Strike => "-".into(),
        Open::Link(u) => format!("|{}]", u),
    };
    let leaf = |t: &str, m: &Marks, _fresh: bool| {
        if m.code {
            return format!("{{{{{}}}}}", t);
        }
        if m.link.as_deref() == Some(t) && !m.strong && !m.em && !m.strike {
            return format!("[{}]", t);
        }
        if escape {
            t.chars().map(|c| if WIKI_SPECIAL.contains(c) { format!("\\{}", c) } else { c.to_string() }).collect()
        } else {
            t.to_string()
        }
    };
    let raw = |_: Flavor, r: &str| r.to_string();
    emit_spans(&hoist_ws(runs), false, &Style { want: &want, open: &open, close: &close, leaf: &leaf, raw: &raw, brk: "\n" })
}

/// The shortest wiki for these runs that reads back as the same runs.
fn wiki_inline_best(runs: &[Inline], cell: bool, force_esc: bool) -> String {
    let n = norm_inl(runs.to_vec());
    if !force_esc {
        let cand = wiki_inline_emit(&n, false);
        if wiki_inlines(&cand) == n && (!cell || split_wiki_cells(&cand, "|").len() == 1) {
            return cand;
        }
    }
    wiki_inline_emit(&n, true)
}

// ---- wiki markup: blocks ----

pub struct Seg {
    block: Block,
    start: usize,
    end: usize,
}

const WIKI_MACROS: [&str; 8] = ["code", "noformat", "quote", "panel", "info", "note", "warning", "tip"];

fn wiki_heading(l: &str) -> Option<(usize, &str)> {
    let b = l.as_bytes();
    (b.len() >= 4 && b[0] == b'h' && (b'1'..=b'6').contains(&b[1]) && b[2] == b'.' && b[3] == b' ').then(|| ((b[1] - b'0') as usize, &l[4..]))
}

fn wiki_list_line(l: &str) -> Option<(Vec<char>, &str)> {
    let n = l.chars().take_while(|c| matches!(c, '*' | '#' | '-')).count();
    (n > 0 && l[n..].starts_with(' ')).then(|| (l[..n].chars().collect(), &l[n + 1..]))
}

fn macro_name(l: &str) -> Option<&str> {
    let r = l.strip_prefix('{')?;
    let end = r.find([':', '}'])?;
    let name = &r[..end];
    (WIKI_MACROS.contains(&name) && l.ends_with('}')).then_some(name)
}

fn wiki_list(entries: &[(Vec<char>, String)], depth: usize) -> Option<Block> {
    let ordered = entries[0].0[depth] == '#';
    let mut items = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let (mk, txt) = &entries[i];
        if mk.contains(&'-') || (mk[depth] == '#') != ordered {
            return None;
        }
        let mut item = Vec::new();
        if mk.len() == depth + 1 {
            item.push(Block::Para(wiki_inlines(txt)));
            i += 1;
        }
        let j = i + entries[i..].iter().take_while(|e| e.0.len() > depth + 1).count();
        if j > i {
            item.push(wiki_list(&entries[i..j], depth + 1)?);
            i = j;
        }
        items.push(item);
    }
    Some(Block::List { ordered, start: None, items })
}

fn wiki_starts_block(l: &str) -> bool {
    wiki_heading(l).is_some() || l.starts_with("bq. ") || l == "----" || wiki_list_line(l).is_some() || l.starts_with('|') || (l.starts_with('{') && macro_name(l).is_some())
}

pub fn wiki_blocks(src: &str) -> Vec<Seg> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut pos = 0;
    for l in src.split('\n') {
        lines.push((pos, l));
        pos += l.len() + 1;
    }
    let end_of = |k: usize| lines[k].0 + lines[k].1.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (st, l) = lines[i];
        if l.trim().is_empty() {
            i += 1;
            continue;
        }
        let mut seg = |block: Block, last: usize, i: &mut usize| {
            out.push(Seg { block, start: st, end: end_of(last) });
            *i = last + 1;
        };
        if let Some(name) = macro_name(l) {
            let closing = format!("{{{}}}", name);
            if let Some(j) = (i + 1..lines.len()).find(|&j| lines[j].1 == closing) {
                let inner = lines[i + 1..j].iter().map(|x| x.1).collect::<Vec<_>>();
                let block = if l == closing {
                    let text = inner.join("\n");
                    match name {
                        "code" => Block::Code { lang: String::new(), text },
                        "noformat" => Block::Raw(Flavor::Wiki, src[st..end_of(j)].to_string()),
                        "quote" => Block::Quote(wiki_blocks(&text).into_iter().map(|s| s.block).collect()),
                        _ => Block::Panel { kind: name.to_string(), body: wiki_blocks(&text).into_iter().map(|s| s.block).collect() },
                    }
                } else if let (Some(lang), "code") = (l.strip_prefix("{code:").and_then(|r| r.strip_suffix('}')), name) {
                    if lang.contains(['|', '=', '}']) {
                        Block::Raw(Flavor::Wiki, src[st..end_of(j)].to_string())
                    } else {
                        Block::Code { lang: lang.to_string(), text: inner.join("\n") }
                    }
                } else {
                    Block::Raw(Flavor::Wiki, src[st..end_of(j)].to_string())
                };
                seg(block, j, &mut i);
                continue;
            }
        }
        if let Some((n, rest)) = wiki_heading(l) {
            seg(Block::Heading(n, wiki_inlines(rest)), i, &mut i);
            continue;
        }
        if let Some(r) = l.strip_prefix("bq. ") {
            seg(Block::Quote(vec![Block::Para(wiki_inlines(r))]), i, &mut i);
            continue;
        }
        if l == "----" {
            seg(Block::Rule, i, &mut i);
            continue;
        }
        if wiki_list_line(l).is_some() {
            let mut entries = Vec::new();
            let mut j = i;
            while j < lines.len() {
                match wiki_list_line(lines[j].1) {
                    Some((m, t)) => entries.push((m, t.to_string())),
                    None => break,
                }
                j += 1;
            }
            let block = wiki_list(&entries, 0).unwrap_or_else(|| Block::Raw(Flavor::Wiki, src[st..end_of(j - 1)].to_string()));
            seg(block, j - 1, &mut i);
            continue;
        }
        if l.starts_with('|') {
            let mut j = i;
            while j < lines.len() && lines[j].1.starts_with('|') {
                j += 1;
            }
            let block = wiki_table(&lines[i..j].iter().map(|x| x.1).collect::<Vec<_>>()).unwrap_or_else(|| Block::Raw(Flavor::Wiki, src[st..end_of(j - 1)].to_string()));
            seg(block, j - 1, &mut i);
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && !lines[j].1.trim().is_empty() && !wiki_starts_block(lines[j].1) {
            j += 1;
        }
        let text = lines[i..j].iter().map(|x| x.1).collect::<Vec<_>>().join("\n");
        seg(Block::Para(wiki_inlines(&text)), j - 1, &mut i);
    }
    out
}

fn wiki_table(rows: &[&str]) -> Option<Block> {
    let mut parsed: Vec<(bool, Vec<Vec<Inline>>)> = Vec::new();
    for r in rows {
        let head = r.starts_with("||");
        let (inner, sep) = if head {
            (r.strip_prefix("||")?.strip_suffix("||")?, "||")
        } else {
            (r.strip_prefix('|')?.strip_suffix('|')?, "|")
        };
        if r.len() < 2 + head as usize * 2 {
            return None;
        }
        let cells = split_wiki_cells(inner, sep).iter().map(|c| wiki_inlines(c.trim())).collect();
        parsed.push((head, cells));
    }
    let heads = parsed.iter().filter(|r| r.0).count();
    if heads > 1 || (heads == 1 && !parsed[0].0) {
        return None;
    }
    let header = parsed[0].0.then(|| parsed[0].1.clone());
    let skip = header.is_some() as usize;
    Some(Block::Table { header, rows: parsed.into_iter().skip(skip).map(|r| r.1).collect() })
}

fn wiki_list_lines(ordered: bool, items: &[Vec<Block>], prefix: &str, force: bool, out: &mut Vec<String>) {
    let marker = format!("{}{}", prefix, if ordered { '#' } else { '*' });
    for it in items {
        let mut lead = false;
        for b in it {
            match b {
                Block::Para(i) if !lead => {
                    out.push(format!("{} {}", marker, wiki_inline_best(i, false, force)));
                    lead = true;
                }
                Block::List { ordered, items, .. } => wiki_list_lines(*ordered, items, &marker, force, out),
                other => out.push(format!("{} {}", marker, emit_wiki_block(other, force).replace('\n', " "))),
            }
        }
    }
}

fn wiki_kind(k: &str) -> &str {
    match k {
        "info" | "note" | "warning" | "tip" | "panel" => k,
        "success" => "tip",
        "error" => "warning",
        _ => "info",
    }
}

fn emit_wiki_block(b: &Block, force: bool) -> String {
    match b {
        Block::Para(i) => wiki_inline_best(i, false, force),
        Block::Heading(n, i) => format!("h{}. {}", n, wiki_inline_best(i, false, force)),
        Block::List { ordered, items, .. } => {
            let mut v = Vec::new();
            wiki_list_lines(*ordered, items, "", force, &mut v);
            v.join("\n")
        }
        Block::Code { lang, text } => {
            let open = if lang.is_empty() { "{code}".to_string() } else { format!("{{code:{}}}", lang) };
            if text.is_empty() {
                format!("{}\n{{code}}", open)
            } else {
                format!("{}\n{}\n{{code}}", open, text)
            }
        }
        Block::Quote(bs) => match bs.as_slice() {
            [Block::Para(i)] if !i.contains(&Inline::Break) => format!("bq. {}", wiki_inline_best(i, false, force)),
            _ => format!("{{quote}}\n{}\n{{quote}}", wiki_join(bs, force)),
        },
        Block::Rule => "----".into(),
        Block::Table { header, rows } => {
            let cell = |c: &Vec<Inline>| {
                let t = wiki_inline_best(c, true, force);
                if t.is_empty() {
                    " ".to_string()
                } else {
                    t
                }
            };
            let mut v = Vec::new();
            if let Some(h) = header {
                v.push(format!("||{}||", h.iter().map(cell).collect::<Vec<_>>().join("||")));
            }
            for r in rows {
                v.push(format!("|{}|", r.iter().map(cell).collect::<Vec<_>>().join("|")));
            }
            v.join("\n")
        }
        Block::Panel { kind, body } => {
            let k = wiki_kind(kind);
            format!("{{{}}}\n{}\n{{{}}}", k, wiki_join(body, force), k)
        }
        Block::Raw(Flavor::Wiki, r) => r.clone(),
        Block::Raw(_, r) => format!("{{noformat}}\n{}\n{{noformat}}", r),
    }
}

fn wiki_join(bs: &[Block], force: bool) -> String {
    bs.iter().map(|b| emit_wiki_block(b, force)).collect::<Vec<_>>().join("\n\n")
}

fn wiki_block(b: &Block) -> String {
    let c = emit_wiki_block(b, false);
    if matches!(b, Block::Raw(..)) {
        return c;
    }
    let segs = wiki_blocks(&c);
    if segs.len() == 1 && segs[0].block == *b && segs[0].start == 0 && segs[0].end == c.len() {
        c
    } else {
        emit_wiki_block(b, true)
    }
}

// ---- public: wiki <-> text ----

/// Jira wiki markup as editable text.
pub fn wiki_to_text(w: &str) -> String {
    let w = w.replace("\r\n", "\n");
    let w = w.trim();
    if w.is_empty() {
        return String::new();
    }
    // blocks separated by exactly one blank line are independent; anything
    // tighter (heading straight into text, say) travels together as raw wiki
    let mut groups: Vec<(usize, usize, Vec<Block>)> = Vec::new();
    for seg in wiki_blocks(w) {
        match groups.last_mut() {
            Some(g) if &w[g.1..seg.start] != "\n\n" => {
                g.1 = seg.end;
                g.2.push(seg.block);
            }
            _ => groups.push((seg.start, seg.end, vec![seg.block])),
        }
    }
    let mut blocks = Vec::new();
    for (a, b, mut bs) in groups {
        let src = &w[a..b];
        let block = if bs.len() == 1 { bs.remove(0) } else { Block::Raw(Flavor::Wiki, src.to_string()) };
        let t = emit_block(&block);
        blocks.push(if text_to_wiki(&t) == src { block } else { Block::Raw(Flavor::Wiki, src.to_string()) });
    }
    let text = join_blocks(&blocks, false);
    if text_to_wiki(&text) != w {
        return emit_block(&Block::Raw(Flavor::Wiki, w.to_string()));
    }
    text
}

/// Editable text back to Jira wiki markup.
pub fn text_to_wiki(text: &str) -> String {
    parse_blocks(text).iter().map(wiki_block).collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(content: &str) -> Value {
        json::parse(&format!(r#"{{"version":1,"type":"doc","content":[{}]}}"#, content)).unwrap()
    }

    fn adf_rt(d: &Value) -> String {
        let t = adf_to_text(d);
        assert_eq!(norm(&text_to_adf(&t)), norm(d), "text was:\n{}", t);
        t
    }

    fn wiki_rt(w: &str) -> String {
        let t = wiki_to_text(w);
        assert_eq!(text_to_wiki(&t), w.replace("\r\n", "\n").trim(), "text was:\n{}", t);
        t
    }

    #[test]
    fn adf_common_blocks_stay_readable() {
        let d = doc(r#"{"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Plan"}]},
            {"type":"paragraph","content":[{"type":"text","text":"Do "},{"type":"text","text":"this","marks":[{"type":"strong"}]},{"type":"text","text":" and "},{"type":"text","text":"that","marks":[{"type":"em"},{"type":"strong"}]},{"type":"hardBreak"},{"type":"text","text":"x","marks":[{"type":"link","attrs":{"href":"http://a/b(c)"}}]},{"type":"text","text":" 2*3 [x] `y`"}]},
            {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"a"}]},{"type":"orderedList","attrs":{"order":3},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"b"}]}]}]}]},{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"c","marks":[{"type":"code"}]}]}]}]},
            {"type":"codeBlock","attrs":{"language":"rs"},"content":[{"type":"text","text":"let a = 1;\n\n```\nb"}]},
            {"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"q1"}]},{"type":"paragraph","content":[{"type":"text","text":"q2"}]}]},
            {"type":"rule"},
            {"type":"panel","attrs":{"panelType":"info"},"content":[{"type":"paragraph","content":[{"type":"text","text":"note"}]}]},
            {"type":"table","content":[{"type":"tableRow","content":[{"type":"tableHeader","content":[{"type":"paragraph","content":[{"type":"text","text":"h|1"}]}]}]},{"type":"tableRow","content":[{"type":"tableCell","content":[{"type":"paragraph","content":[{"type":"text","text":"v"}]}]}]}]}"#);
        let t = adf_rt(&d);
        assert!(!t.contains("adf-json"), "{}", t);
        assert!(t.starts_with("## Plan\n\nDo **this** and ***that***\\\n[x](http://a/b(c\\)) 2\\*3 \\[x] \\`y\\`\n\n- a\n  3. b\n- `c`\n"), "{}", t);
    }

    #[test]
    fn adf_oddities_go_raw_but_survive() {
        let d = doc(r##"{"type":"paragraph","content":[{"type":"mention","attrs":{"id":"1","text":"@Ann"}},{"type":"text","text":" hi","marks":[{"type":"textColor","attrs":{"color":"#f00"}}]}]},
            {"type":"mediaSingle","attrs":{"layout":"center"},"content":[{"type":"media","attrs":{"id":"x","type":"file","collection":"c"}}]},
            {"type":"paragraph"},
            {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"a"}]}]}]},
            {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"b"}]}]}]},
            {"type":"taskList","attrs":{"localId":"1"},"content":[{"type":"taskItem","attrs":{"localId":"2","state":"TODO"},"content":[{"type":"text","text":"t"}]}]}"##);
        adf_rt(&d);
        adf_rt(&doc(""));
    }

    #[test]
    fn adf_normalisation_is_semantic() {
        let a = doc(r#"{"type":"paragraph","content":[{"type":"text","text":"a","marks":[{"type":"strong"},{"type":"em"}]},{"type":"text","text":"b","marks":[{"type":"em"},{"type":"strong"}]}]},{"type":"orderedList","attrs":{"order":1},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"x"}]}]}]}"#);
        let t = adf_rt(&a);
        assert!(!t.contains("adf-json"), "{}", t);
    }

    #[test]
    fn text_escapes_block_markers() {
        for src in ["# not a heading", "- not a list", "1. not a list", "> quote", "---", "```", "|a|", ":::x", " lead", "a\\", "<adf>x</adf>", "~~x~~", "*x*", "[a](b)", "`a`", "snake_case"] {
            let d = doc(&format!(r#"{{"type":"paragraph","content":[{{"type":"text","text":{}}}]}}"#, quote(src)));
            let t = adf_rt(&d);
            assert!(!t.contains("adf-json"), "{} -> {}", src, t);
        }
    }

    #[test]
    fn wiki_basics() {
        let t = wiki_rt("h2. Title\n\nSome *bold* and _it_ -gone- {{mono}} [link|http://x] [http://y] snake_case 2*3*4\n\n* one\n** two\n* x\n*# three\n\n# n1\n## n2\n\n{code:java}\nint x;\n\nint y;\n{code}\n\nbq. quoted\n\n----\n\n||h1||h2||\n|a|[b|http://c]|\n\n{quote}\nA\n\nB\n{quote}\n\n{info}\ntext\n{info}");
        assert!(!t.contains("wiki-raw"), "{}", t);
        assert!(t.contains("## Title") || t.contains("Title"));
    }

    #[test]
    fn wiki_odd_shapes_survive() {
        for w in [
            "h1. Tight\ntext right after",
            "a\n\n\n\nb",
            "  leading\n\ntrailing  ",
            "\\*not bold\\* and \\_x\\_ and \\\\ and {color:red}red{color}",
            "{noformat}\n* x\n\nh1. y\n{noformat}",
            "{panel:title=T}\nbody\n{panel}",
            "* a\n- b\n# c",
            "| a | b |\n| c | d |",
            "|a|b",
            "||a|b||",
            "{code}\n{code}",
            "{{unterminated and [broken|",
            "line one\nline two\n\n[~user] [PROJ-1] !img.png! +u+ ^s^ ~sub~ ??cite??",
            "* *bold item*\n* item with | pipe",
            "|*b*|{{c}}|\\|esc|",
            "h7. nope\n\nbq.nospace\n\n#nospace",
            "日本語 *太字* ünïcode",
            "a\r\nb\r\n\r\nc",
            "{quote}\nnever closed",
            "```\nfenced in wiki\n```\n\n<adf>x</adf> <!---->",
            "*a **b** c*",
            "[a|b|c]",
            "* \n*",
        ] {
            wiki_rt(w);
        }
        assert_eq!(wiki_to_text(""), "");
    }

    #[test]
    fn wiki_fuzz() {
        let frags = ["h1. ", "h2.", "* ", "** ", "# ", "#* ", "- ", "bq. ", "----", "|", "||", "{code}", "{code:rs}", "{quote}", "{info}", "{noformat}", "{{", "}}", "[", "]", "|", "*", "_", "-", "+", "^", "~", "\\", "\n", "\n\n", "\n", " ", "a", "b c", "http://x.y", "é", "!", "?", "{", "}", "`", "```", "<adf>", "</adf>", ":::", "> ", "1. ", "\r\n"];
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        for _ in 0..4000 {
            let mut w = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let n = 1 + (seed % 14) as usize;
            for _ in 0..n {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                w.push_str(frags[(seed % frags.len() as u64) as usize]);
            }
            let t = wiki_to_text(&w);
            assert_eq!(text_to_wiki(&t), w.replace("\r\n", "\n").trim(), "wiki {:?} via text {:?}", w, t);
        }
    }

    #[test]
    fn adf_fuzz() {
        // random documents built from the dialect's own pieces plus oddities
        let texts = ["a", " b ", "*", "`", "[", "]", "(", ")", "\\", "~~", "<adf>", "- x", "1. x", "# h", "|", "é", "a b", "  "];
        let marks = ["", "strong", "em", "strike", "code", "link", "strong,em", "em,link", "underline"];
        let mut seed: u64 = 0x1234_5678_9abc_def1;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..3000 {
            let mut inl = Vec::new();
            for _ in 0..1 + next(5) {
                if next(8) == 0 {
                    inl.push(r#"{"type":"hardBreak"}"#.to_string());
                    continue;
                }
                let ms: Vec<String> = marks[next(marks.len())]
                    .split(',')
                    .filter(|m| !m.is_empty())
                    .map(|m| match m {
                        "link" => r#"{"type":"link","attrs":{"href":"http://u/(x) y"}}"#.to_string(),
                        "underline" => r#"{"type":"underline"}"#.to_string(),
                        m => format!(r#"{{"type":"{}"}}"#, m),
                    })
                    .collect();
                inl.push(format!(r#"{{"type":"text","text":{},"marks":[{}]}}"#, quote(texts[next(texts.len())]), ms.join(",")));
            }
            let para = format!(r#"{{"type":"paragraph","content":[{}]}}"#, inl.join(","));
            let wrapped = match next(5) {
                0 => format!(r#"{{"type":"bulletList","content":[{{"type":"listItem","content":[{}]}}]}}"#, para),
                1 => format!(r#"{{"type":"blockquote","content":[{}]}}"#, para),
                2 => format!(r#"{{"type":"heading","attrs":{{"level":3}},"content":[{}]}}"#, inl.join(",")),
                3 => format!(r#"{{"type":"table","content":[{{"type":"tableRow","content":[{{"type":"tableCell","content":[{}]}}]}}]}}"#, para),
                _ => para,
            };
            let d = doc(&format!("{},{}", wrapped, wrapped));
            adf_rt(&d);
        }
    }

    #[test]
    fn typed_text_to_adf_and_wiki() {
        let t = "# Title\n\nSome **bold** and `code` with [a link](http://x).\\\nsecond line\n\n- one\n  - nested\n- two\n\n1. first\n2. second\n\n```rs\nfn main() {}\n```\n\n> quoted\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n:::info\nheads up\n:::";
        let w = text_to_wiki(t);
        assert_eq!(w, "h1. Title\n\nSome *bold* and {{code}} with [a link|http://x]\nsecond line\n\n* one\n** nested\n* two\n\n# first\n# second\n\n{code:rs}\nfn main() {}\n{code}\n\nbq. quoted\n\n||a||b||\n|1|2|\n\n{info}\nheads up\n{info}".replace("link|http://x]\nsecond", "link|http://x].\nsecond"));
        let a = text_to_adf(t);
        assert_eq!(a.get("content").arr().len(), 8);
        assert_eq!(adf_to_text(&a), t);
    }
}
