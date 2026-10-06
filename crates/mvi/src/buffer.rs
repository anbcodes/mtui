// Text buffer: a Vec of lines plus an edit-log based undo history.

use crate::diag::Diag;
use crate::syntax::{Highlighter, Lang};
use std::io::Write;
use std::path::{Path, PathBuf};

pub type Pos = (usize, usize); // (line, byte col)

#[derive(Clone, Debug)]
enum Edit {
    Ins(Pos, String),
    Del(Pos, String),
}

#[derive(Default)]
struct Group {
    edits: Vec<Edit>,
    cursor: Pos,
}

pub struct Buffer {
    pub lines: Vec<String>,
    pub path: Option<PathBuf>,
    pub name: String,
    pub dirty: bool,
    pub crlf: bool,
    pub lang: &'static Lang,
    pub hl: Highlighter,
    pub diags: Vec<Diag>,
    pub expand_tab: bool,
    pub indent_w: usize,
    // view state
    pub cy: usize,
    pub cx: usize,
    pub want_x: usize,
    pub top: usize,
    pub left: usize,
    pub marks: [Option<Pos>; 26],
    pub version: u64,
    undo: Vec<Group>,
    redo: Vec<Group>,
    cur: Group,
}

pub fn end_of(p: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => (p.0, p.1 + text.len()),
        Some(i) => (p.0 + text.matches('\n').count(), text.len() - i - 1),
    }
}

impl Buffer {
    pub fn new(path: Option<PathBuf>) -> Buffer {
        let mut b = Buffer {
            lines: vec![String::new()],
            name: path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "[scratch]".into()),
            path,
            dirty: false,
            crlf: false,
            lang: crate::syntax::plain(),
            hl: Highlighter::default(),
            diags: Vec::new(),
            expand_tab: true,
            indent_w: 4,
            cy: 0,
            cx: 0,
            want_x: 0,
            top: 0,
            left: 0,
            marks: [None; 26],
            version: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            cur: Group { edits: Vec::new(), cursor: (usize::MAX, 0) },
        };
        if let Some(p) = b.path.clone() {
            if let Ok(bytes) = std::fs::read(&p) {
                let text = String::from_utf8_lossy(&bytes);
                b.crlf = text.contains("\r\n");
                let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
                if lines.len() > 1 && lines.last().map_or(false, |l| l.is_empty()) {
                    lines.pop();
                }
                b.lines = lines;
            }
            let first = b.lines.first().cloned().unwrap_or_default();
            b.lang = crate::syntax::detect(&p, &first);
            b.detect_indent();
        }
        b
    }

    pub fn canonical(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
    }

    fn detect_indent(&mut self) {
        let (mut tabs, mut spaces) = (0, 0);
        let mut min_sp = usize::MAX;
        for l in self.lines.iter().take(2000) {
            if l.starts_with('\t') {
                tabs += 1;
            } else if l.starts_with("  ") {
                spaces += 1;
                let n = l.len() - l.trim_start_matches(' ').len();
                if n >= 2 && !l.trim_start().starts_with('*') {
                    min_sp = min_sp.min(n);
                }
            }
        }
        if tabs > spaces {
            self.expand_tab = false;
            self.indent_w = 4;
        } else {
            self.expand_tab = true;
            self.indent_w = if min_sp == usize::MAX { self.lang.indent } else { min_sp.min(8) };
        }
    }

    pub fn save(&mut self) -> std::io::Result<usize> {
        let path = self.path.clone().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no file name"))?;
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut data = String::with_capacity(self.lines.iter().map(|l| l.len() + 2).sum());
        for l in &self.lines {
            data.push_str(l);
            data.push_str(nl);
        }
        let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if is_link || !path.exists() {
            std::fs::write(&path, &data)?;
        } else {
            // write to temp file then rename for atomicity, keeping permissions
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let tmp = dir.join(format!(".{}.mvi~", path.file_name().unwrap().to_string_lossy()));
            let res = (|| {
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(data.as_bytes())?;
                f.sync_all()?;
                if let Ok(m) = std::fs::metadata(&path) {
                    let _ = std::fs::set_permissions(&tmp, m.permissions());
                }
                std::fs::rename(&tmp, &path)
            })();
            if res.is_err() {
                let _ = std::fs::remove_file(&tmp);
                std::fs::write(&path, &data)?;
            }
        }
        self.dirty = false;
        Ok(data.len())
    }

    pub fn line_len(&self, l: usize) -> usize {
        self.lines.get(l).map_or(0, |s| s.len())
    }

    fn touch(&mut self, l: usize) {
        self.dirty = true;
        self.version += 1;
        self.hl.invalidate(l);
    }

    fn raw_insert(&mut self, (l, c): Pos, text: &str) -> Pos {
        self.touch(l);
        if !text.contains('\n') {
            self.lines[l].insert_str(c, text);
            return (l, c + text.len());
        }
        let tail = self.lines[l].split_off(c);
        let mut parts = text.split('\n');
        self.lines[l].push_str(parts.next().unwrap());
        let mut new: Vec<String> = parts.map(|s| s.to_string()).collect();
        let n = new.len();
        let last_len = new[n - 1].len();
        new[n - 1].push_str(&tail);
        self.lines.splice(l + 1..l + 1, new);
        (l + n, last_len)
    }

    fn raw_delete(&mut self, a: Pos, b: Pos) -> String {
        self.touch(a.0);
        if a.0 == b.0 {
            return self.lines[a.0].drain(a.1..b.1).collect();
        }
        let mut s = String::from(&self.lines[a.0][a.1..]);
        s.push('\n');
        for i in a.0 + 1..b.0 {
            s.push_str(&self.lines[i]);
            s.push('\n');
        }
        s.push_str(&self.lines[b.0][..b.1]);
        let tail = self.lines[b.0][b.1..].to_string();
        self.lines[a.0].truncate(a.1);
        self.lines[a.0].push_str(&tail);
        self.lines.drain(a.0 + 1..=b.0);
        s
    }

    pub fn insert(&mut self, p: Pos, text: &str) -> Pos {
        if text.is_empty() {
            return p;
        }
        self.redo.clear();
        self.shift_diags(p, text.matches('\n').count() as isize);
        self.cur.edits.push(Edit::Ins(p, text.to_string()));
        self.raw_insert(p, text)
    }

    pub fn delete(&mut self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let b = (b.0.min(self.lines.len() - 1), b.1);
        let b = (b.0, b.1.min(self.line_len(b.0)));
        if a >= b {
            return String::new();
        }
        self.redo.clear();
        self.shift_diags(a, -((b.0 - a.0) as isize));
        let s = self.raw_delete(a, b);
        self.cur.edits.push(Edit::Del(a, s.clone()));
        s
    }

    fn shift_diags(&mut self, p: Pos, dl: isize) {
        if dl == 0 {
            return;
        }
        for d in &mut self.diags {
            if d.line > p.0 {
                d.line = (d.line as isize + dl).max(p.0 as isize) as usize;
            }
        }
    }

    pub fn text(&self, a: Pos, b: Pos) -> String {
        if a.0 == b.0 {
            return self.lines[a.0][a.1..b.1].to_string();
        }
        let mut s = String::from(&self.lines[a.0][a.1..]);
        for i in a.0 + 1..b.0 {
            s.push('\n');
            s.push_str(&self.lines[i]);
        }
        s.push('\n');
        s.push_str(&self.lines[b.0][..b.1]);
        s
    }

    /// Close the current undo group. `cursor` is where to return on undo.
    pub fn commit(&mut self, cursor: Pos) {
        if !self.cur.edits.is_empty() {
            let mut g = std::mem::take(&mut self.cur);
            if g.cursor == (usize::MAX, 0) {
                g.cursor = cursor;
            }
            self.undo.push(g);
            if self.undo.len() > 1000 {
                self.undo.remove(0);
            }
        }
        self.cur.cursor = (usize::MAX, 0);
    }

    /// Remember the cursor before a change begins.
    pub fn mark_start(&mut self, cursor: Pos) {
        if self.cur.edits.is_empty() {
            self.cur.cursor = cursor;
        }
    }

    pub fn undo(&mut self) -> Option<Pos> {
        let g = self.undo.pop()?;
        for e in g.edits.iter().rev() {
            match e {
                Edit::Ins(p, t) => {
                    let end = end_of(*p, t);
                    self.raw_delete(*p, end);
                }
                Edit::Del(p, t) => {
                    self.raw_insert(*p, t);
                }
            }
        }
        let c = if g.cursor.0 == usize::MAX { g.edits.first().map(|e| match e { Edit::Ins(p, _) | Edit::Del(p, _) => *p }).unwrap_or((0, 0)) } else { g.cursor };
        self.redo.push(g);
        Some(c)
    }

    pub fn redo(&mut self) -> Option<Pos> {
        let g = self.redo.pop()?;
        let mut last = (0, 0);
        for e in &g.edits {
            match e {
                Edit::Ins(p, t) => {
                    self.raw_insert(*p, t);
                    last = *p;
                }
                Edit::Del(p, t) => {
                    let end = end_of(*p, t);
                    self.raw_delete(*p, end);
                    last = *p;
                }
            }
        }
        self.undo.push(g);
        Some(last)
    }

    pub fn indent_unit(&self) -> String {
        if self.expand_tab {
            " ".repeat(self.indent_w)
        } else {
            "\t".into()
        }
    }
}
