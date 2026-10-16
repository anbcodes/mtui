// Suggestions: proposed edits and comments that sit beside a file in a
// `FILE.suggest` text file, like suggestion mode in Google Docs. A reviewer
// (usually an AI) writes small changes with a reason; the author accepts
// (applies) or rejects (drops) each one.
//
// File format, one block per suggestion, blocks separated by blank lines:
//
//     @ 12:34            line 12, column 34 (1-based, in characters); ":34" is optional
//     - old text         the text the suggestion is about (repeat for more lines)
//     + new text         what it becomes ("+" alone deletes the text)
//     > why              the comment (repeat for more lines)
//
// With `+` lines the block is a change; with only `-` lines it is a comment
// on that text; with only `@` and `>` it is a comment on the line. A `+`
// without `-` inserts at the column (the end of the line without one). One
// space after the marker is dropped. Lines starting with `#` are ignored.
//
// The `-` text is what anchors a suggestion: if the file has changed since,
// it is searched for near the stated line, and a suggestion whose text is
// gone is shown as out of date. mvi keeps positions current while editing
// and rewrites the .suggest file whenever the main file is written.

use crate::buffer::{end_of, Buffer, Pos};
use crate::editor::Editor;
use crate::picker::{PickItem, Picker};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub line: usize,
    /// Byte column in the line.
    pub col: usize,
    pub old: String,
    /// `None`: a comment, nothing to apply.
    pub new: Option<String>,
    pub comment: String,
}

#[derive(Debug, PartialEq)]
struct Raw {
    line: usize,
    col: Option<usize>,
    old: Option<String>,
    new: Option<String>,
    comment: String,
}

pub fn suggest_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".suggest");
    path.with_file_name(name)
}

fn strip_marker(l: &str) -> &str {
    let r = &l[1..];
    r.strip_prefix(' ').unwrap_or(r)
}

fn parse_raw(text: &str) -> Vec<Raw> {
    fn flush(cur: &mut Option<Raw>, olds: &mut Vec<String>, news: &mut Vec<String>, notes: &mut Vec<String>, out: &mut Vec<Raw>) {
        if let Some(mut r) = cur.take() {
            r.old = (!olds.is_empty()).then(|| olds.join("\n"));
            r.new = (!news.is_empty()).then(|| news.join("\n"));
            r.comment = notes.join("\n");
            out.push(r);
        }
        olds.clear();
        news.clear();
        notes.clear();
    }
    let (mut out, mut cur) = (Vec::new(), None::<Raw>);
    let (mut olds, mut news, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for l in text.lines() {
        let l = l.strip_suffix('\r').unwrap_or(l);
        if l.trim().is_empty() {
            flush(&mut cur, &mut olds, &mut news, &mut notes, &mut out);
        } else if l.starts_with('#') {
        } else if l.starts_with('@') {
            flush(&mut cur, &mut olds, &mut news, &mut notes, &mut out);
            let spec = l[1..].split_whitespace().next().unwrap_or("");
            let (a, b) = spec.split_once(':').unwrap_or((spec, ""));
            if let Ok(n) = a.parse::<usize>() {
                cur = Some(Raw { line: n.max(1), col: b.parse().ok().filter(|&c: &usize| c >= 1), old: None, new: None, comment: String::new() });
            }
        } else if cur.is_some() {
            match l.as_bytes()[0] {
                b'-' => olds.push(strip_marker(l).to_string()),
                b'+' => news.push(strip_marker(l).to_string()),
                b'>' => notes.push(strip_marker(l).to_string()),
                _ => notes.push(l.to_string()),
            }
        }
    }
    flush(&mut cur, &mut olds, &mut news, &mut notes, &mut out);
    out
}

/// Does `old` sit at `p`?
pub fn matches_at(lines: &[String], (l, c): Pos, old: &str) -> bool {
    let Some(line) = lines.get(l) else { return false };
    if c > line.len() || !line.is_char_boundary(c) {
        return false;
    }
    let parts: Vec<&str> = old.split('\n').collect();
    if parts.len() == 1 {
        return line[c..].starts_with(old);
    }
    let last = l + parts.len() - 1;
    if last >= lines.len() || &line[c..] != parts[0] {
        return false;
    }
    (1..parts.len() - 1).all(|k| lines[l + k] == parts[k]) && lines[last].starts_with(parts[parts.len() - 1])
}

/// Find `old` for a suggestion stated at `line` (and maybe byte column
/// `col`): at the column, else on that line, else on the nearest line
/// within 200.
fn locate(lines: &[String], line: usize, col: Option<usize>, old: &str) -> Option<Pos> {
    let first = old.split('\n').next().unwrap_or("");
    let multi = old.contains('\n');
    let on_line = |l: usize, near: usize| -> Option<Pos> {
        let s = lines.get(l)?;
        let mut cands: Vec<usize> = if multi { s.ends_with(first).then(|| s.len() - first.len()).into_iter().collect() } else { s.match_indices(first).map(|m| m.0).collect() };
        cands.retain(|&c| matches_at(lines, (l, c), old));
        cands.into_iter().min_by_key(|&c| c.abs_diff(near)).map(|c| (l, c))
    };
    if let Some(c) = col {
        if matches_at(lines, (line, c), old) {
            return Some((line, c));
        }
    }
    if let Some(p) = on_line(line, col.unwrap_or(0)) {
        return Some(p);
    }
    (1..=200).find_map(|d| on_line(line.wrapping_sub(d), 0).or_else(|| on_line(line + d, 0)))
}

fn byte_of(lines: &[String], line: usize, col: usize) -> usize {
    let s = &lines[line];
    s.char_indices().nth(col).map_or(s.len(), |(i, _)| i)
}

fn resolve(raw: Raw, lines: &[String]) -> Suggestion {
    let line = (raw.line - 1).min(lines.len() - 1);
    let col = raw.col.map(|c| byte_of(lines, line, c - 1));
    let old = raw.old.unwrap_or_default();
    let (line, col) = if old.is_empty() {
        (line, col.unwrap_or(if raw.new.is_some() { lines[line].len() } else { 0 }))
    } else {
        locate(lines, line, col, &old).unwrap_or((line, col.unwrap_or(0)))
    };
    Suggestion { line, col, old, new: raw.new, comment: raw.comment }
}

pub fn parse(text: &str, lines: &[String]) -> Vec<Suggestion> {
    let mut v: Vec<Suggestion> = parse_raw(text).into_iter().map(|r| resolve(r, lines)).collect();
    v.sort_by_key(|s| (s.line, s.col));
    v
}

fn push_lines(out: &mut String, marker: char, text: &str) {
    for l in text.split('\n') {
        out.push(marker);
        if !l.is_empty() {
            out.push(' ');
            out.push_str(l);
        }
        out.push('\n');
    }
}

pub fn serialize(sugs: &[Suggestion], lines: &[String]) -> String {
    let mut out = String::from("# mvi suggestions. @ LINE:COL, then - old text, + new text (empty to delete), > comment.\n# Blank line between suggestions. mvi: ga accept, gx reject, ]x [x next/previous.\n");
    for s in sugs {
        let col = lines.get(s.line).map_or(s.col, |l| l[..s.col.min(l.len())].chars().count()) + 1;
        out.push_str(&format!("\n@ {}:{}\n", s.line + 1, col));
        if !s.old.is_empty() {
            push_lines(&mut out, '-', &s.old);
        }
        if let Some(n) = &s.new {
            push_lines(&mut out, '+', n);
        }
        if !s.comment.is_empty() {
            push_lines(&mut out, '>', &s.comment);
        }
    }
    out
}

impl Suggestion {
    pub fn end(&self) -> Pos {
        end_of((self.line, self.col), &self.old)
    }

    /// The text it is about is no longer where it says.
    pub fn stale(&self, lines: &[String]) -> bool {
        !self.old.is_empty() && !matches_at(lines, (self.line, self.col), &self.old)
    }

    /// One-line summary: `"old" → "new": comment`.
    pub fn summary(&self) -> String {
        let flat = |s: &str| s.replace('\n', "↵");
        let what = match &self.new {
            Some(n) if n.is_empty() => format!("delete \"{}\"", flat(&self.old)),
            Some(n) if self.old.is_empty() => format!("insert \"{}\"", flat(n)),
            Some(n) => format!("\"{}\" → \"{}\"", flat(&self.old), flat(n)),
            None if self.old.is_empty() => "comment".to_string(),
            None => format!("comment on \"{}\"", flat(&self.old)),
        };
        if self.comment.is_empty() {
            what
        } else {
            format!("{}: {}", what, self.comment.replace('\n', " "))
        }
    }
}

/// Keep positions right across an insertion of `text` at `p`.
pub fn shift_insert(sugs: &mut [Suggestion], p: Pos, text: &str) {
    let n = text.matches('\n').count();
    for s in sugs.iter_mut().filter(|s| (s.line, s.col) >= p) {
        if s.line > p.0 {
            s.line += n;
        } else if n == 0 {
            s.col += text.len();
        } else {
            s.col = s.col - p.1 + text.len() - text.rfind('\n').unwrap() - 1;
            s.line += n;
        }
    }
}

/// Keep positions right across the deletion of `a..b`.
pub fn shift_delete(sugs: &mut [Suggestion], a: Pos, b: Pos) {
    for s in sugs.iter_mut().filter(|s| (s.line, s.col) > a) {
        if (s.line, s.col) < b {
            (s.line, s.col) = a;
        } else if s.line == b.0 {
            s.col = s.col - b.1 + a.1;
            s.line = a.0;
        } else {
            s.line -= b.0 - a.0;
        }
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Buffer {
    /// Read `FILE.suggest` (when it exists) into `sugs`.
    pub fn load_suggest(&mut self) {
        let Some(p) = self.path.as_ref().map(|p| suggest_path(p)) else { return };
        self.sug_stamp = mtime(&p);
        self.sugs = match std::fs::read(&p) {
            Ok(b) => parse(&String::from_utf8_lossy(&b), &self.lines),
            Err(_) => Vec::new(),
        };
    }

    /// Pick up edits made to the .suggest file by someone else.
    pub fn reload_suggest_if_changed(&mut self) -> bool {
        let Some(p) = self.path.as_ref().map(|p| suggest_path(p)) else { return false };
        if mtime(&p) == self.sug_stamp {
            return false;
        }
        self.load_suggest();
        true
    }

    /// Re-find suggestions whose text moved, then write the .suggest file
    /// (removing it when nothing is left).
    pub fn save_suggest(&mut self) {
        let Some(p) = self.path.as_ref().map(|p| suggest_path(p)) else { return };
        for s in &mut self.sugs {
            if s.stale(&self.lines) {
                let col = self.lines.get(s.line).map(|l| l[..s.col.min(l.len())].chars().count());
                if let Some(q) = locate(&self.lines, s.line, col.map(|c| byte_of(&self.lines, s.line, c)), &s.old) {
                    (s.line, s.col) = q;
                }
            }
        }
        self.sugs.sort_by_key(|s| (s.line, s.col));
        if self.sugs.is_empty() {
            let _ = std::fs::remove_file(&p);
        } else {
            let tmp = p.with_extension("suggest~");
            let text = serialize(&self.sugs, &self.lines);
            if std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &p)).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        self.sug_stamp = mtime(&p);
    }

    /// The suggestion for the cursor: one covering it, else the first that
    /// starts or ends on its line.
    pub fn sug_at(&self, (cy, cx): Pos) -> Option<usize> {
        let touches = |s: &Suggestion| s.line <= cy && s.end().0 >= cy;
        self.sugs.iter().position(|s| touches(s) && (s.line, s.col) <= (cy, cx) && (cy, cx) <= s.end()).or_else(|| self.sugs.iter().position(touches))
    }
}

impl Editor {
    pub fn poll_suggest(&mut self) {
        for b in &mut self.bufs {
            b.reload_suggest_if_changed();
        }
    }

    /// The message-line text for the suggestion under the cursor.
    pub fn suggest_message(&self) -> Option<String> {
        let b = self.bb();
        let s = &b.sugs[b.sug_at(self.cursor())?];
        Some(if s.stale(&b.lines) {
            format!("out of date: {}  (gx drop)", s.summary())
        } else if s.new.is_some() {
            format!("{}  (ga accept, gx reject)", s.summary())
        } else {
            format!("{}  (gx dismiss)", s.summary())
        })
    }

    /// `K` on a line with suggestions: all of them, with full comments.
    pub fn suggest_details(&self) -> Vec<String> {
        let cy = self.cursor().0;
        let mut out = Vec::new();
        for s in self.bb().sugs.iter().filter(|s| s.line <= cy && s.end().0 >= cy) {
            out.push(s.summary().lines().next().unwrap_or("").to_string());
            out.extend(s.comment.lines().map(|l| format!("    {}", l)));
        }
        out
    }

    pub fn jump_suggest(&mut self, fwd: bool, n: usize) {
        for _ in 0..n.max(1) {
            let c = self.cursor();
            let v: Vec<Pos> = self.bb().sugs.iter().map(|s| (s.line, s.col)).collect();
            if v.is_empty() {
                return self.info("no suggestions");
            }
            let t = if fwd { v.iter().find(|&&p| p > c).or(v.first()) } else { v.iter().rev().find(|&&p| p < c).or(v.last()) };
            let (y, x) = *t.unwrap();
            self.last_jump = c;
            self.set_cursor((y, x.min(self.bb().lines[y].len().saturating_sub(1))));
        }
        let th = self.text_h();
        let b = self.b();
        if b.cy < b.top || b.cy >= b.top + th {
            b.top = b.cy.saturating_sub(th / 2);
        }
    }

    /// Apply suggestion `i` (a comment is just dropped). False if its text
    /// has changed since.
    fn apply_suggest(&mut self, i: usize) -> bool {
        let b = self.b();
        if b.sugs[i].stale(&b.lines) {
            return false;
        }
        let s = b.remove_sug(i);
        if let Some(n) = &s.new {
            let a = (s.line, s.col);
            b.delete(a, s.end());
            b.insert(a, n);
        }
        true
    }

    /// `ga` (`all`: `:accept!`): apply the suggestion at the cursor.
    pub fn suggest_accept(&mut self, all: bool) {
        let c = self.cursor();
        self.b().mark_start(c);
        let (mut done, mut skipped) = (0, 0);
        if all {
            // from the end, so that earlier positions stay put
            let mut i = self.bb().sugs.len();
            while i > 0 {
                i -= 1;
                if self.bb().sugs[i].new.is_none() {
                    continue;
                }
                if self.apply_suggest(i) {
                    done += 1;
                } else {
                    skipped += 1;
                }
            }
        } else {
            let Some(i) = self.bb().sug_at(c) else { return self.info("no suggestion here") };
            if self.apply_suggest(i) {
                done = 1;
            } else {
                skipped = 1;
            }
        }
        if done > 0 {
            self.b().save_suggest();
            self.after_change();
        }
        match (done, skipped) {
            (_, 0) => self.info(format!("{} accepted", done)),
            (0, _) => self.err("out of date: the text has changed (gx to drop it)"),
            _ => self.err(format!("{} accepted, {} out of date", done, skipped)),
        }
    }

    /// `gx` (`all`: `:reject!`): drop the suggestion at the cursor.
    pub fn suggest_reject(&mut self, all: bool) {
        let c = self.cursor();
        self.b().mark_start(c);
        let n = if all {
            let n = self.bb().sugs.len();
            while !self.bb().sugs.is_empty() {
                self.b().remove_sug(0);
            }
            n
        } else {
            let Some(i) = self.bb().sug_at(c) else { return self.info("no suggestion here") };
            self.b().remove_sug(i);
            1
        };
        self.b().save_suggest();
        self.after_change();
        self.info(format!("{} rejected", n));
    }

    /// `:suggest`: every suggestion in the buffer, in a picker.
    pub fn open_suggest_picker(&mut self) {
        let b = self.bb();
        let items: Vec<PickItem> = b
            .sugs
            .iter()
            .map(|s| {
                let col = b.lines.get(s.line).map_or(0, |l| l[..s.col.min(l.len())].chars().count());
                let mark = if s.stale(&b.lines) { "!" } else if s.new.is_some() { "±" } else { "?" };
                PickItem { label: format!("{:>5}: {} {}", s.line + 1, mark, s.summary()), path: None, buf: Some(self.cur), line: s.line, col }
            })
            .collect();
        if items.is_empty() {
            return self.info("no suggestions");
        }
        self.picker = Some(Picker::new("suggestions", items));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split('\n').map(|l| l.to_string()).collect()
    }

    #[test]
    fn parse_forms() {
        let ls = lines("Hello, world.\nsecond line\nthird");
        let t = "# c\n@ 1:6\n- ,\n+\n> no comma\n> before world\n\n@ 2\n- second\n> reword?\n\n@ 3\n+ !\n\n@ 1\n> a note\n";
        let v = parse(t, &ls);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], Suggestion { line: 0, col: 0, old: String::new(), new: None, comment: "a note".into() });
        assert_eq!((v[1].line, v[1].col, v[1].old.as_str(), v[1].new.as_deref()), (0, 5, ",", Some("")));
        assert_eq!(v[1].comment, "no comma\nbefore world");
        assert_eq!((v[2].line, v[2].col, v[2].new.clone()), (1, 0, None));
        assert_eq!((v[3].line, v[3].col, v[3].new.as_deref()), (2, 5, Some("!")));
    }

    #[test]
    fn locate_moved_text() {
        let ls = lines("a\nb\nfoo bar\nc");
        let v = parse("@ 1:1\n- bar\n+ baz\n", &ls);
        assert_eq!((v[0].line, v[0].col), (2, 4));
        let v = parse("@ 9\n- gone\n+ x\n", &ls);
        assert!(v[0].stale(&ls));
    }

    #[test]
    fn multi_line_roundtrip() {
        let ls = lines("one two\nthree four\nfive");
        let t = "@ 1:5\n- two\n- three\n+ 2 3\n> join\n";
        let v = parse(t, &ls);
        assert_eq!((v[0].old.as_str(), v[0].new.as_deref()), ("two\nthree", Some("2 3")));
        assert!(!v[0].stale(&ls));
        assert_eq!(v[0].end(), (1, 5));
        assert_eq!(parse(&serialize(&v, &ls), &ls), v);
    }

    #[test]
    fn positions_follow_edits() {
        let mut v = vec![Suggestion { line: 3, col: 5, old: "x".into(), new: None, comment: String::new() }];
        shift_insert(&mut v, (1, 0), "ab\ncd");
        assert_eq!((v[0].line, v[0].col), (4, 5));
        shift_insert(&mut v, (4, 2), "ab\ncd");
        assert_eq!((v[0].line, v[0].col), (5, 5));
        shift_delete(&mut v, (4, 1), (5, 2));
        assert_eq!((v[0].line, v[0].col), (4, 4));
        shift_delete(&mut v, (4, 3), (4, 6));
        assert_eq!((v[0].line, v[0].col), (4, 3));
    }

    #[test]
    fn accept_reject_and_sync() {
        let dir = std::env::temp_dir().join(format!("mvi-sug-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.md");
        std::fs::write(&f, "Hello, world.\nit could of been\nend\n").unwrap();
        let sf = suggest_path(&f);
        std::fs::write(&sf, "@ 1:6\n- ,\n+\n> why\n\n@ 2\n- could of\n+ could have\n").unwrap();
        let mut e = Editor::new(80, 24);
        e.open_file(&f);
        assert_eq!(e.bb().sugs.len(), 2);
        // accept the second, then reject the first; the file follows
        e.set_cursor((1, 4));
        e.suggest_accept(false);
        assert_eq!(e.bb().lines[1], "it could have been");
        assert_eq!(e.bb().sugs.len(), 1);
        assert!(std::fs::read_to_string(&sf).unwrap().contains("@ 1:6"));
        // editing above moves the anchor; saving rewrites the file with the new line
        e.b().insert((0, 0), "x\n");
        e.b().commit((0, 0));
        e.ex("w");
        assert!(std::fs::read_to_string(&sf).unwrap().contains("@ 2:6"));
        e.set_cursor((1, 5));
        e.suggest_accept(false);
        assert_eq!(e.bb().lines[1], "Hello world.");
        assert!(e.bb().sugs.is_empty());
        e.ex("w");
        assert!(!sf.exists());
        // undo brings the suggestion (and the file) back, redo drops it again
        e.b().undo();
        assert_eq!(e.bb().lines[1], "Hello, world.");
        assert_eq!(e.bb().sugs.len(), 1);
        assert!(std::fs::read_to_string(&sf).unwrap().contains("@ 2:6"));
        e.b().redo();
        assert!(e.bb().sugs.is_empty() && !sf.exists());
        e.b().undo();
        e.set_cursor((1, 5));
        e.suggest_reject(false);
        assert!(e.bb().sugs.is_empty() && !sf.exists());
        e.b().undo();
        assert_eq!(e.bb().sugs.len(), 1);
        assert!(sf.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
