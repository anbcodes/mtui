// A small markdown renderer: text in, styled and word-wrapped lines out. It
// covers what READMEs, issues and pull requests actually use: headings,
// emphasis, inline code, links, images, quotes, nested lists with task
// boxes, fenced code (syntax highlighted), tables, rules and a little HTML.
// Callers draw the lines themselves, and decide what to do with images.

use crate::screen::{char_width, Style, BOLD, ITALIC, UNDERLINE};
use crate::syntax::{self, State};
use crate::wrap::str_width;

pub struct Opts {
    pub width: usize,
    /// Report images as their own lines (`Line::image`). When off they show
    /// as `[alt]` text.
    pub images: bool,
    /// Treat single newlines in a paragraph as line breaks (GitHub comments
    /// do; README files don't).
    pub hard_breaks: bool,
}

pub type Span = (String, Style);

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// 0-based line of the source this came from.
    pub src: usize,
    /// Columns of quote bars / list bullets before the content.
    pub indent: usize,
    /// (alt, url) when this line stands for an image: draw it at `indent`.
    pub image: Option<(String, String)>,
}

impl Line {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.0.as_str()).collect()
    }
    fn blank(src: usize) -> Line {
        Line { spans: Vec::new(), src, indent: 0, image: None }
    }
    fn is_blank(&self) -> bool {
        self.image.is_none() && self.spans.iter().all(|s| s.0.trim().is_empty() && s.1.bg == 0)
    }
}

const B: u8 = 1;
const I: u8 = 2;
const S: u8 = 4;
const C: u8 = 8;
const L: u8 = 16;

const FG_DIM: u8 = 246;
const BG_CODE: u8 = 235;

fn style(f: u8) -> Style {
    let (mut fg, mut bg, mut attr) = (0, 0, 0);
    if f & B != 0 {
        attr |= BOLD;
    }
    if f & I != 0 {
        attr |= ITALIC;
    }
    if f & S != 0 {
        fg = FG_DIM;
    }
    if f & C != 0 {
        fg = 180;
        bg = 236;
    }
    if f & L != 0 {
        fg = 75;
        attr |= UNDERLINE;
    }
    Style::new(fg, bg, attr)
}

enum Seg {
    Text(String, u8),
    Img(String, String),
    Break,
}

const TAGS: &[&str] = &[
    "a", "p", "br", "b", "i", "em", "strong", "code", "pre", "details", "summary", "div", "span", "img", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "tr", "td", "th", "thead", "tbody", "blockquote", "hr", "sup", "sub", "kbd", "picture", "source", "center", "strike", "del", "ins", "u",
];

/// `[text](url)` at the start of `s`: (text, url, bytes used).
fn parse_link(s: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0;
    let mut end = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let e = end?;
    let rest = s[e + 1..].strip_prefix('(')?;
    let mut depth = 1;
    for (j, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let url = rest[..j].split_whitespace().next().unwrap_or("");
                    return Some((&s[1..e], url.trim_matches(['<', '>']), e + 2 + j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let p = tag.find(&format!("{}=", name))? + name.len() + 1;
    let q = tag[p..].chars().next()?;
    if q != '"' && q != '\'' {
        return Some(tag[p..].split_whitespace().next()?.to_string());
    }
    let v = &tag[p + 1..];
    Some(v[..v.find(q)?].to_string())
}

fn inline(s: &str, f: u8, out: &mut Vec<Seg>) {
    let mut buf = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Seg::Text(std::mem::take(&mut buf), f));
            }
        };
    }
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap();
        match c {
            '\\' if rest[1..].chars().next().is_some_and(|n| n.is_ascii_punctuation()) => {
                let n = rest[1..].chars().next().unwrap();
                buf.push(n);
                i += 1 + n.len_utf8();
            }
            '`' => {
                let n = rest.chars().take_while(|&x| x == '`').count();
                let close = "`".repeat(n);
                match rest[n..].find(&close) {
                    Some(j) => {
                        flush!();
                        out.push(Seg::Text(rest[n..n + j].trim().to_string(), f | C));
                        i += n + j + n;
                    }
                    None => {
                        buf.push_str(&close);
                        i += n;
                    }
                }
            }
            '!' if rest.starts_with("![") => match parse_link(&rest[1..]) {
                Some((alt, url, len)) => {
                    flush!();
                    out.push(Seg::Img(alt.to_string(), url.to_string()));
                    i += 1 + len;
                }
                None => {
                    buf.push('!');
                    i += 1;
                }
            },
            '[' => match parse_link(rest) {
                Some((text, _, len)) => {
                    flush!();
                    inline(text, f | L, out);
                    i += len;
                }
                None => {
                    buf.push('[');
                    i += 1;
                }
            },
            '*' | '_' | '~' => {
                let n = rest.chars().take_while(|&x| x == c).count();
                let (d, flag) = match (c, n) {
                    ('~', 1) => (1, 0),
                    ('~', _) => (2, S),
                    (_, 1) => (1, I),
                    _ => (2, B),
                };
                let delim = c.to_string().repeat(d);
                let prev_alnum = s[..i].chars().next_back().is_some_and(|x| x.is_alphanumeric());
                let open = flag != 0 && !(c == '_' && prev_alnum) && rest[d..].chars().next().is_some_and(|x| !x.is_whitespace());
                let close = if open {
                    rest[d..].match_indices(&delim).map(|(p, _)| p + d).find(|&p| {
                        let before_ok = p > d && !rest[..p].ends_with(char::is_whitespace);
                        let after = rest[p + d..].chars().next();
                        let lone = d == 2 || (after != Some(c) && !rest[..p].ends_with(c));
                        let word_end = c != '_' || !after.is_some_and(|x| x.is_alphanumeric());
                        before_ok && lone && word_end
                    })
                } else {
                    None
                };
                match close {
                    Some(p) => {
                        flush!();
                        inline(&rest[d..p], f | flag, out);
                        i += p + d;
                    }
                    None => {
                        buf.push_str(&delim);
                        i += d;
                    }
                }
            }
            '<' => {
                if rest.starts_with("<!--") {
                    i += rest.find("-->").map_or(rest.len(), |e| e + 3);
                    continue;
                }
                let end = rest.find('>').filter(|&e| e < 600);
                let tag = end.map(|e| &rest[1..e]).unwrap_or("");
                let name = tag.trim_start_matches('/').split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("").to_ascii_lowercase();
                match end {
                    Some(e) if tag.starts_with("http://") || tag.starts_with("https://") => {
                        flush!();
                        out.push(Seg::Text(tag.to_string(), f | L));
                        i += e + 1;
                    }
                    Some(e) if name == "br" => {
                        flush!();
                        out.push(Seg::Break);
                        i += e + 1;
                    }
                    Some(e) if name == "img" => {
                        flush!();
                        if let Some(src) = attr(tag, "src") {
                            out.push(Seg::Img(attr(tag, "alt").unwrap_or_default(), src));
                        }
                        i += e + 1;
                    }
                    Some(e) if TAGS.contains(&name.as_str()) => i += e + 1,
                    _ => {
                        buf.push('<');
                        i += 1;
                    }
                }
            }
            'h' if (rest.starts_with("http://") || rest.starts_with("https://")) && !s[..i].chars().next_back().is_some_and(|x| x.is_alphanumeric()) => {
                let e = rest.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(rest.len());
                let url = rest[..e].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
                flush!();
                out.push(Seg::Text(url.to_string(), f | L));
                i += url.len();
            }
            _ => {
                buf.push(c);
                i += c.len_utf8();
            }
        }
    }
    flush!();
}

fn is_badge(url: &str) -> bool {
    let u = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    u.ends_with(".svg") || url.contains("shields.io") || url.contains("badgen.net") || url.contains("/badge")
}

fn push_span(v: &mut Vec<Span>, text: &str, st: Style) {
    match v.last_mut() {
        Some((t, s)) if *s == st => t.push_str(text),
        _ => v.push((text.to_string(), st)),
    }
}

/// Greedy word wrap of styled text. Words longer than a line are split.
fn wrap_spans(spans: &[Span], width: usize) -> Vec<Vec<Span>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut cw = 0;
    let mut word: Vec<Span> = Vec::new();
    let mut ww = 0;
    let mut space = false;
    let flush_word = |word: &mut Vec<Span>, ww: &mut usize, space: &mut bool, cur: &mut Vec<Span>, cw: &mut usize, lines: &mut Vec<Vec<Span>>| {
        if word.is_empty() {
            return;
        }
        let need = *ww + usize::from(*space && *cw > 0);
        if *cw + need > width && *cw > 0 {
            lines.push(std::mem::take(cur));
            *cw = 0;
        }
        if *space && *cw > 0 {
            push_span(cur, " ", Style::default());
            *cw += 1;
        }
        if *ww <= width {
            for (t, st) in word.drain(..) {
                push_span(cur, &t, st);
            }
            *cw += *ww;
        } else {
            // longer than a line: split by characters
            for (t, st) in word.drain(..) {
                for ch in t.chars() {
                    let w1 = char_width(ch);
                    if *cw + w1 > width {
                        lines.push(std::mem::take(cur));
                        *cw = 0;
                    }
                    let mut b = [0u8; 4];
                    push_span(cur, ch.encode_utf8(&mut b), st);
                    *cw += w1;
                }
            }
        }
        *ww = 0;
        *space = false;
    };
    for (t, st) in spans {
        for ch in t.chars() {
            if ch == ' ' || ch == '\t' || ch == '\n' {
                flush_word(&mut word, &mut ww, &mut space, &mut cur, &mut cw, &mut lines);
                space = true;
            } else {
                push_span(&mut word, ch.encode_utf8(&mut [0u8; 4]), *st);
                ww += char_width(ch);
            }
        }
    }
    flush_word(&mut word, &mut ww, &mut space, &mut cur, &mut cw, &mut lines);
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn prefix(lines: Vec<Line>, first: &[Span], rest: &[Span]) -> Vec<Line> {
    let w1: usize = first.iter().map(|s| str_width(&s.0)).sum();
    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut l)| {
            let p = if i == 0 { first } else { rest };
            let blank = l.is_blank();
            let mut spans = p.to_vec();
            if !blank || l.image.is_some() {
                spans.extend(l.spans);
            }
            l.spans = spans;
            l.indent += w1;
            l
        })
        .collect()
}

struct Ctx<'a> {
    o: &'a Opts,
}

fn is_fence(t: &str) -> Option<(char, usize, &str)> {
    let t = t.trim_start();
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    (n >= 3).then(|| (c, n, t[n..].trim()))
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let t = t.trim_start();
    let n = t.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&n).then(|| t[n..].strip_prefix(' ').map(|r| (n, r.trim().trim_end_matches('#').trim_end()))).flatten().or_else(|| (n >= 1 && n <= 6 && t.len() == n).then_some((n, "")))
}

fn is_rule(t: &str) -> bool {
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && ["-", "*", "_"].iter().any(|m| t.chars().all(|c| m.starts_with(c)))
}

/// A list marker: (indent, marker text, content column, content).
fn list_item(l: &str) -> Option<(usize, String, usize, &str)> {
    let ind = l.len() - l.trim_start().len();
    let t = &l[ind..];
    let (m, rest) = if let Some(r) = t.strip_prefix(['-', '*', '+']).filter(|r| r.starts_with(' ') || r.is_empty()) {
        ("•".to_string(), r)
    } else {
        let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if d == 0 || d > 9 || !t[d..].starts_with(['.', ')']) || !(t[d + 1..].starts_with(' ') || t.len() == d + 1) {
            return None;
        }
        (format!("{}.", &t[..d]), &t[d + 1..])
    };
    let content = rest.trim_start();
    Some((ind, m, ind + (t.len() - rest.len()) + (rest.len() - content.len()), content))
}

fn table_sep(l: &str) -> bool {
    let t = l.trim().trim_matches('|');
    !t.is_empty() && t.contains('-') && t.split('|').all(|c| {
        let c = c.trim();
        !c.is_empty() && c.chars().all(|x| x == '-' || x == ':') && c.contains('-')
    })
}

fn cells(l: &str) -> Vec<String> {
    let t = l.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in t.chars() {
        if esc {
            if c != '|' {
                out.last_mut().unwrap().push('\\');
            }
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '|' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out.into_iter().map(|c| c.trim().to_string()).collect()
}

fn plain(s: &str) -> String {
    let mut segs = Vec::new();
    inline(s, 0, &mut segs);
    segs.iter()
        .map(|g| match g {
            Seg::Text(t, _) => t.clone(),
            Seg::Img(a, _) => format!("[{}]", if a.is_empty() { "image" } else { a }),
            Seg::Break => " ".to_string(),
        })
        .collect()
}

fn truncate(s: &str, w: usize) -> String {
    if str_width(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut cw = 0;
    for c in s.chars() {
        if cw + char_width(c) + 1 > w {
            break;
        }
        out.push(c);
        cw += char_width(c);
    }
    out.push('…');
    out
}

impl Ctx<'_> {
    fn flow(&self, text_lines: &[(usize, &str)], w: usize, out: &mut Vec<Line>) {
        let src = text_lines.first().map_or(0, |l| l.0);
        let mut segs: Vec<Seg> = Vec::new();
        for (n, (_, l)) in text_lines.iter().enumerate() {
            if n > 0 {
                if self.o.hard_breaks {
                    segs.push(Seg::Break);
                } else {
                    segs.push(Seg::Text(" ".into(), 0));
                }
            }
            inline(l.trim(), 0, &mut segs);
        }
        let mut group: Vec<Span> = Vec::new();
        let emit = |group: &mut Vec<Span>, out: &mut Vec<Line>| {
            let g = std::mem::take(group);
            if g.iter().all(|s| s.0.trim().is_empty()) {
                return;
            }
            for spans in wrap_spans(&g, w) {
                out.push(Line { spans, src, indent: 0, image: None });
            }
        };
        for seg in segs {
            match seg {
                Seg::Text(t, f) => push_span(&mut group, &t, style(f)),
                Seg::Break => emit(&mut group, out),
                Seg::Img(alt, url) => {
                    if self.o.images && !is_badge(&url) && url.starts_with("http") {
                        emit(&mut group, out);
                        out.push(Line { spans: vec![(alt.clone(), style(L))], src, indent: 0, image: Some((alt, url)) });
                    } else {
                        push_span(&mut group, &format!("[{}]", if alt.is_empty() { "image" } else { &alt }), style(L));
                    }
                }
            }
        }
        emit(&mut group, out);
    }

    fn code(&self, lines: &[(usize, &str)], lang: &str, w: usize, out: &mut Vec<Line>) {
        let lang = match lang.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str() {
            "bash" | "zsh" | "shell" | "console" => "sh".to_string(),
            l => l.to_string(),
        };
        let lg = syntax::by_name(&lang).unwrap_or_else(syntax::plain);
        let (mut st, mut hl) = (State::Normal, Vec::new());
        let bg = |s: Style| Style::new(s.fg, BG_CODE, s.attr);
        for &(n, l) in lines {
            let t = l.replace('\t', "    ");
            st = syntax::highlight(lg, &t, st, &mut hl);
            // runs of one highlight class
            let mut runs: Vec<Span> = Vec::new();
            for (bi, ch) in t.char_indices() {
                push_span(&mut runs, ch.encode_utf8(&mut [0u8; 4]), bg(syntax::style(*hl.get(bi).unwrap_or(&0))));
            }
            // chunk to the width, then pad each row so the block is a solid bar
            let mut rows: Vec<Vec<Span>> = vec![Vec::new()];
            let mut cw = 0;
            for (text, s) in runs {
                for ch in text.chars() {
                    let w1 = char_width(ch);
                    if cw + w1 > w.saturating_sub(2) {
                        rows.push(Vec::new());
                        cw = 0;
                    }
                    push_span(rows.last_mut().unwrap(), ch.encode_utf8(&mut [0u8; 4]), s);
                    cw += w1;
                }
            }
            for mut r in rows {
                let used: usize = r.iter().map(|s| str_width(&s.0)).sum();
                let pad = Style::new(0, BG_CODE, 0);
                r.insert(0, (" ".into(), pad));
                r.push((" ".repeat(w.saturating_sub(used + 1)), pad));
                out.push(Line { spans: r, src: n, indent: 0, image: None });
            }
        }
    }

    fn table(&self, rows: &[(usize, &str)], w: usize, out: &mut Vec<Line>) {
        let mut grid: Vec<Vec<String>> = rows.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, r)| cells(r.1).iter().map(|c| plain(c)).collect()).collect();
        let n = grid.iter().map(|r| r.len()).max().unwrap_or(0);
        for r in grid.iter_mut() {
            r.resize(n, String::new());
        }
        let mut cw: Vec<usize> = (0..n).map(|c| grid.iter().map(|r| str_width(&r[c])).max().unwrap_or(1).max(1)).collect();
        let seps = 3 * n.saturating_sub(1);
        while cw.iter().sum::<usize>() + seps > w && cw.iter().any(|&x| x > 3) {
            let i = (0..n).max_by_key(|&i| cw[i]).unwrap();
            cw[i] -= 1;
        }
        let dim = Style::fg(FG_DIM);
        let draw = |cols: &[String], bold: bool, out: &mut Vec<Line>, src: usize| {
            let mut spans: Vec<Span> = Vec::new();
            for (i, c) in cols.iter().enumerate() {
                let t = truncate(c, cw[i]);
                let pad = cw[i].saturating_sub(str_width(&t));
                if i > 0 {
                    spans.push((" │ ".into(), dim));
                }
                spans.push((format!("{}{}", t, " ".repeat(pad)), if bold { Style::new(0, 0, BOLD) } else { Style::default() }));
            }
            out.push(Line { spans, src, indent: 0, image: None });
        };
        let mut it = grid.iter().enumerate();
        if let Some((_, h)) = it.next() {
            draw(h, true, out, rows[0].0);
            let rule: Vec<Span> = cw.iter().enumerate().flat_map(|(i, &x)| [(if i > 0 { "─┼─".to_string() } else { String::new() }, dim), ("─".repeat(x), dim)]).collect();
            out.push(Line { spans: rule, src: rows[0].0, indent: 0, image: None });
        }
        for (i, r) in it {
            draw(r, false, out, rows[(i + 1).min(rows.len() - 1)].0);
        }
    }

    fn blocks(&self, lines: &[(usize, &str)], w: usize) -> Vec<Line> {
        let mut out: Vec<Line> = Vec::new();
        let gap = |out: &mut Vec<Line>, src: usize| {
            if !out.is_empty() && !out.last().unwrap().is_blank() {
                out.push(Line::blank(src));
            }
        };
        let mut i = 0;
        let mut after_item = false;
        while i < lines.len() {
            let (n, l) = lines[i];
            let t = l.trim();
            let was_item = std::mem::take(&mut after_item);
            if t.is_empty() {
                if !out.is_empty() && !out.last().unwrap().is_blank() {
                    out.push(Line::blank(n));
                }
                i += 1;
            } else if t.starts_with("<!--") {
                while i < lines.len() && !lines[i].1.contains("-->") {
                    i += 1;
                }
                i += 1;
            } else if let Some((c, k, lang)) = is_fence(l) {
                let mut j = i + 1;
                while j < lines.len() && !(is_fence(lines[j].1).is_some_and(|(c2, k2, rest)| c2 == c && k2 >= k && rest.is_empty())) {
                    j += 1;
                }
                gap(&mut out, n);
                self.code(&lines[i + 1..j.min(lines.len())], lang, w, &mut out);
                i = j + 1;
            } else if let Some((lv, text)) = heading(l) {
                gap(&mut out, n);
                let f = match lv {
                    1 => Style::new(75, 0, BOLD | UNDERLINE),
                    2 => Style::new(75, 0, BOLD),
                    _ => Style::new(110, 0, BOLD),
                };
                let mut segs = Vec::new();
                inline(text, 0, &mut segs);
                let spans: Vec<Span> = segs.into_iter().filter_map(|g| if let Seg::Text(t, fl) = g { Some((t, Style::new(f.fg, if fl & C != 0 { 236 } else { 0 }, f.attr))) } else { None }).collect();
                for spans in wrap_spans(&spans, w) {
                    out.push(Line { spans, src: n, indent: 0, image: None });
                }
                i += 1;
            } else if is_rule(t) && !t.starts_with(['*', '-']) || (is_rule(t) && list_item(l).is_none()) {
                gap(&mut out, n);
                out.push(Line { spans: vec![("─".repeat(w), Style::fg(FG_DIM))], src: n, indent: 0, image: None });
                i += 1;
            } else if t.starts_with('>') {
                let mut inner = Vec::new();
                while i < lines.len() && lines[i].1.trim_start().starts_with('>') {
                    let s = lines[i].1.trim_start()[1..].strip_prefix(' ').unwrap_or(&lines[i].1.trim_start()[1..]);
                    inner.push((lines[i].0, s));
                    i += 1;
                }
                gap(&mut out, n);
                let sub = self.blocks(&inner, w.saturating_sub(2).max(1));
                let bar = [("▎ ".to_string(), Style::fg(FG_DIM))];
                out.extend(prefix(sub, &bar, &bar));
            } else if i + 1 < lines.len() && t.contains('|') && table_sep(lines[i + 1].1) {
                let mut j = i + 2;
                while j < lines.len() && !lines[j].1.trim().is_empty() && lines[j].1.contains('|') {
                    j += 1;
                }
                gap(&mut out, n);
                self.table(&lines[i..j], w, &mut out);
                i = j;
            } else if let Some((ind, marker, col, content)) = list_item(l) {
                // the item: its first line and what's indented under it
                let mut item: Vec<(usize, String)> = vec![(n, content.to_string())];
                let mut j = i + 1;
                while j < lines.len() {
                    let lj = lines[j].1;
                    let lead = lj.len() - lj.trim_start().len();
                    if lj.trim().is_empty() {
                        // a blank line continues the item only if more indented text follows
                        match lines[j + 1..].iter().find(|x| !x.1.trim().is_empty()) {
                            Some(nx) if nx.1.len() - nx.1.trim_start().len() > ind && list_item(nx.1).is_none_or(|x| x.0 > ind) => {
                                item.push((lines[j].0, String::new()));
                                j += 1;
                            }
                            _ => break,
                        }
                    } else if lead > ind {
                        item.push((lines[j].0, lj[lead.min(col)..].to_string()));
                        j += 1;
                    } else if list_item(lj).is_none() && is_fence(lj).is_none() && heading(lj).is_none() && !lj.trim_start().starts_with('>') && item.len() == 1 {
                        item.push((lines[j].0, lj.trim().to_string()));
                        j += 1;
                    } else {
                        break;
                    }
                }
                // a list right under a paragraph line stays attached to it
                let tight = i > 0 && !lines[i - 1].1.trim().is_empty() && out.last().is_some_and(|l| !l.is_blank());
                if !was_item && !tight {
                    gap(&mut out, n);
                }
                let (mut mark, mut body) = (marker, item);
                for (box_, g) in [("[ ]", "☐"), ("[x]", "☑"), ("[X]", "☑")] {
                    if body[0].1.starts_with(box_) && body[0].1[3..].starts_with(' ') {
                        mark = g.to_string();
                        body[0].1 = body[0].1[3..].trim_start().to_string();
                        break;
                    }
                }
                let refs: Vec<(usize, &str)> = body.iter().map(|(a, b)| (*a, b.as_str())).collect();
                let pw = str_width(&mark) + 1;
                let sub = self.blocks(&refs, w.saturating_sub(pw + ind.min(8)).max(1));
                let pad = " ".repeat(ind.min(8));
                let first = [(format!("{}{} ", pad, mark), Style::fg(FG_DIM))];
                let rest = [(" ".repeat(pad.len() + pw), Style::default())];
                out.extend(prefix(sub, &first, &rest));
                after_item = true;
                i = j;
            } else {
                // paragraph
                let mut j = i;
                while j < lines.len() {
                    let lj = lines[j].1;
                    if lj.trim().is_empty() || (j > i && (is_fence(lj).is_some() || heading(lj).is_some() || lj.trim_start().starts_with('>') || list_item(lj).is_some() || (is_rule(lj.trim()) && !lj.trim().contains(' ')) || lj.trim().starts_with("<!--"))) {
                        break;
                    }
                    if j > i && lj.contains('|') && j + 1 < lines.len() && table_sep(lines[j + 1].1) {
                        break;
                    }
                    j += 1;
                }
                let before = out.len();
                let mut tmp = Vec::new();
                self.flow(&lines[i..j], w, &mut tmp);
                if !tmp.is_empty() {
                    gap(&mut out, n);
                    out.extend(tmp);
                }
                let _ = before;
                i = j.max(i + 1);
            }
        }
        while out.last().is_some_and(|l| l.is_blank()) {
            out.pop();
        }
        out
    }
}

/// Render markdown into lines no wider than `o.width`.
pub fn render(src: &str, o: &Opts) -> Vec<Line> {
    let lines: Vec<(usize, &str)> = src.lines().enumerate().map(|(i, l)| (i, l.trim_end_matches('\r'))).collect();
    Ctx { o }.blocks(&lines, o.width.max(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(src: &str, w: usize) -> Vec<String> {
        render(src, &Opts { width: w, images: true, hard_breaks: false }).iter().map(|l| l.text()).collect()
    }

    #[test]
    fn inline_styles() {
        let l = &render("a **bold** and *it* `code` [link](http://x.io) ~~gone~~ snake_case_name", &Opts { width: 80, images: true, hard_breaks: false })[0];
        assert_eq!(l.text(), "a bold and it code link gone snake_case_name");
        let st = |w: &str| l.spans.iter().find(|s| s.0.contains(w)).unwrap().1;
        assert!(st("bold").attr & BOLD != 0 && st("it").attr & ITALIC != 0);
        assert_eq!(st("code").fg, 180);
        assert!(st("link").attr & UNDERLINE != 0);
    }

    #[test]
    fn headings_and_paragraphs() {
        assert_eq!(r("# Title\ntext one\ntext two\n\n## Sub\nmore", 40), vec!["Title", "", "text one text two", "", "Sub", "", "more"]);
    }

    #[test]
    fn wraps_to_width() {
        let long = "word ".repeat(30) + "a-very-long-word-that-does-not-fit-at-all-on-one-line";
        for l in r(&long, 20) {
            assert!(str_width(&l) <= 20, "{:?}", l);
        }
        assert_eq!(r("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn lists() {
        let s = "- one\n- two wraps around here\n  - nested\n1. first\n2. second\n- [ ] todo\n- [x] done";
        assert_eq!(r(s, 20), vec!["• one", "• two wraps around", "  here", "  • nested", "1. first", "2. second", "☐ todo", "☑ done"]);
    }

    #[test]
    fn quote_and_rule() {
        assert_eq!(r("> quoted *text*\n> more\n\n---\nafter", 30), vec!["▎ quoted text more", "", "──────────────────────────────", "", "after"]);
    }

    #[test]
    fn code_blocks() {
        let out = render("```rust\nfn main() {}\n```", &Opts { width: 20, images: true, hard_breaks: false });
        assert_eq!(out.len(), 1);
        assert_eq!(str_width(&out[0].text()), 20);
        assert!(out[0].spans.iter().any(|s| s.1.fg == 176 && s.0 == "fn"));
        assert!(out[0].spans.iter().all(|s| s.1.bg == BG_CODE));
    }

    #[test]
    fn tables() {
        let t = r("| a | long header |\n|---|:-:|\n| 1 | two |\n| 333 | x |", 40);
        assert_eq!(t, vec!["a   │ long header", "────┼────────────", "1   │ two        ", "333 │ x          "]);
        for l in r("| aaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbbbbb |\n|-|-|\n| c | d |", 20) {
            assert!(str_width(&l) <= 20);
        }
    }

    #[test]
    fn images_and_badges() {
        let o = Opts { width: 40, images: true, hard_breaks: false };
        let out = render("see ![shot](https://x.io/a.png) here\n\n[![ci](https://img.shields.io/b.svg)](http://ci)\n\n<img src=\"https://x.io/c.jpg\" alt=\"c\">", &o);
        let imgs: Vec<_> = out.iter().filter_map(|l| l.image.clone()).collect();
        assert_eq!(imgs, vec![("shot".to_string(), "https://x.io/a.png".to_string()), ("c".to_string(), "https://x.io/c.jpg".to_string())]);
        assert!(out.iter().any(|l| l.text() == "[ci]"));
        let off = render("![shot](https://x.io/a.png)", &Opts { images: false, ..o });
        assert_eq!(off[0].text(), "[shot]");
    }

    #[test]
    fn breaks_and_html() {
        let o = Opts { width: 40, images: false, hard_breaks: true };
        let t: Vec<String> = render("one\ntwo<br>three\n<!-- hidden -->\n<details><summary>More</summary>\ntext\n</details>\nVec<String> stays", &o).iter().map(|l| l.text()).collect();
        assert_eq!(t, vec!["one", "two", "three", "", "More", "text", "Vec<String> stays"]);
    }

    #[test]
    fn source_lines() {
        let out = render("a\n\n# b\n\n- c", &Opts { width: 20, images: false, hard_breaks: false });
        let src: Vec<usize> = out.iter().filter(|l| !l.is_blank()).map(|l| l.src).collect();
        assert_eq!(src, vec![0, 2, 4]);
    }
}
