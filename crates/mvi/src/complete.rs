// Lightweight "intellisense": buffer words, keywords, symbols (with their
// definition line as a signature hint) and file paths.

use crate::buffer::Buffer;
use crate::syntax::Lang;
use mtui::fuzzy::fuzzy;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Symbol,
    Word,
    Keyword,
    Path,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Symbol => "ƒ",
            Kind::Word => "w",
            Kind::Keyword => "k",
            Kind::Path => "/",
        }
    }
}

pub struct Item {
    pub text: String,
    pub kind: Kind,
    pub detail: String,
}

pub struct Completion {
    pub items: Vec<Item>,
    pub sel: Option<usize>,
    pub start: usize, // byte col where the completed word starts
    pub line: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Find definitions in a buffer: (name, line, signature-ish text).
pub fn symbols(lang: &Lang, lines: &[String]) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for (ln, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.is_empty() || t.len() > 400 {
            continue;
        }
        let mut words = t.split(|c: char| !(is_word_char(c) || c == '#' || c == '!')).filter(|w| !w.is_empty()).peekable();
        let mut found = false;
        while let Some(w) = words.next() {
            if lang.defs.contains(&w) {
                if let Some(name) = words.peek() {
                    let name = name.trim_end_matches('!');
                    if !name.is_empty() && !lang.keywords.contains(&name) && !name.chars().next().unwrap().is_ascii_digit() {
                        out.push((name.to_string(), ln, t.trim_end_matches('{').trim().to_string()));
                        found = true;
                    }
                }
                break;
            }
            // only modifiers may precede a def keyword
            if !matches!(w, "pub" | "export" | "async" | "static" | "public" | "private" | "protected" | "default" | "unsafe" | "extern" | "inline" | "abstract" | "final" | "override" | "local" | "const" | "data" | "open" | "declare") {
                break;
            }
        }
        // C-like functions: "type name(" at column 0, not ending in ';'
        if !found && (lang.name == "c" || lang.name == "cpp") && !l.starts_with(char::is_whitespace) && !t.ends_with(';') && !t.starts_with('#') {
            if let Some(p) = t.find('(') {
                let head = &t[..p];
                if let Some(name) = head.split(|c: char| !(is_word_char(c) || c == ':' || c == '~')).filter(|s| !s.is_empty()).last() {
                    if head.trim() != name && !lang.keywords.contains(&name) {
                        out.push((name.to_string(), ln, t.trim_end_matches('{').trim().to_string()));
                    }
                }
            }
        }
    }
    out
}

/// Word before the cursor (start byte col).
pub fn word_start(line: &str, col: usize, path_mode: bool) -> usize {
    let mut s = col;
    for (i, c) in line[..col].char_indices().rev() {
        if is_word_char(c) || (path_mode && (c == '/' || c == '.' || c == '-' || c == '~')) {
            s = i;
        } else {
            break;
        }
    }
    s
}

fn path_items(prefix: &str) -> Vec<Item> {
    let expanded = if let Some(r) = prefix.strip_prefix("~/") {
        format!("{}/{}", std::env::var("HOME").unwrap_or_default(), r)
    } else {
        prefix.to_string()
    };
    let (dir, file) = match expanded.rfind('/') {
        Some(i) => (&expanded[..=i], &expanded[i + 1..]),
        None => ("./", expanded.as_str()),
    };
    let shown_dir = match prefix.rfind('/') {
        Some(i) => &prefix[..=i],
        None => "",
    };
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten().take(2000) {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(file) || (name.starts_with('.') && !file.starts_with('.')) {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            v.push(Item { text: format!("{}{}{}", shown_dir, name, if is_dir { "/" } else { "" }), kind: Kind::Path, detail: String::new() });
        }
    }
    v.sort_by(|a, b| a.text.cmp(&b.text));
    v
}

/// All symbols of all buffers: name -> definition line (signature hint).
pub fn symbol_table(bufs: &[Buffer]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for b in bufs {
        for (n, _, d) in symbols(b.lang, &b.lines) {
            m.entry(n).or_insert(d);
        }
    }
    m
}

pub fn complete(b: &Buffer, line: usize, col: usize, manual: bool, syms: &HashMap<String, String>) -> Option<Completion> {
    let l = &b.lines[line];
    // path completion
    let ps = word_start(l, col, true);
    let pword = &l[ps..col];
    if pword.contains('/') {
        let items = path_items(pword);
        if items.is_empty() || (items.len() == 1 && items[0].text == pword) {
            return None;
        }
        return Some(Completion { items, sel: None, start: ps, line });
    }
    let start = word_start(l, col, false);
    let prefix = &l[start..col];
    if prefix.is_empty() && !manual {
        return None;
    }
    // candidates scored by proximity to the cursor
    let lo = line.saturating_sub(1500);
    let hi = (line + 1500).min(b.lines.len());
    let mut best: HashMap<&str, (i32, Kind)> = HashMap::new();
    for (ln, text) in b.lines[lo..hi].iter().enumerate() {
        let ln = ln + lo;
        let sc = -((ln as i64 - line as i64).unsigned_abs().min(5000) as i32);
        let mut it = text.char_indices().peekable();
        while let Some((s, c)) = it.next() {
            if !(is_word_char(c) && !c.is_ascii_digit()) {
                continue;
            }
            let mut e = s + c.len_utf8();
            while let Some(&(i, c)) = it.peek() {
                if !is_word_char(c) {
                    break;
                }
                e = i + c.len_utf8();
                it.next();
            }
            if (ln == line && s == start) || e - s < 2 {
                continue;
            }
            let w = &text[s..e];
            let ent = best.entry(w).or_insert((i32::MIN, Kind::Word));
            if sc > ent.0 {
                ent.0 = sc;
            }
        }
    }
    for k in syms.keys() {
        best.entry(k.as_str()).or_insert((-6000, Kind::Word));
    }
    for k in b.lang.keywords.iter().chain(b.lang.types).chain(b.lang.consts) {
        best.entry(k).or_insert((-7000, Kind::Keyword));
    }
    let lp = prefix.to_lowercase();
    let mut scored: Vec<(i32, &str, Kind)> = Vec::new();
    for (w, (prox, kind)) in &best {
        if *w == prefix {
            continue;
        }
        let m = if w.starts_with(prefix) {
            Some(3000)
        } else if w.to_lowercase().starts_with(&lp) {
            Some(2000)
        } else if prefix.len() >= 2 {
            fuzzy(prefix, w).map(|s| s * 4)
        } else {
            None
        };
        if let Some(m) = m {
            let kind = if *kind == Kind::Word && syms.contains_key(*w) { Kind::Symbol } else { *kind };
            let kb = if kind == Kind::Symbol { 300 } else { 0 };
            scored.push((m + kb + prox / 20, w, kind));
        }
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    scored.truncate(50);
    let items = scored.into_iter().map(|(_, w, k)| Item { text: w.to_string(), kind: k, detail: syms.get(w).cloned().unwrap_or_default() }).collect();
    Some(Completion { items, sel: None, start, line })
}
