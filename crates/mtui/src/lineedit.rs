// A small text input with emacs-style keys and history. The text may hold
// newlines (from pastes or Alt-Enter); the cursor is a byte offset.

use crate::screen::{Screen, Style};
use crate::term::Key;
use crate::wrap::{str_width, wrap};

#[derive(Default)]
pub struct LineEdit {
    pub text: String,
    pub cur: usize,
    hist: Vec<String>,
    hist_idx: usize,
}

pub enum Edit {
    /// The key changed the text or cursor.
    Handled,
    /// The key isn't an editing key; the caller decides.
    Ignored,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl LineEdit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.cur = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cur = 0;
    }

    /// Clear and remember the text for Up/Down recall.
    pub fn take(&mut self) -> String {
        let t = std::mem::take(&mut self.text);
        self.cur = 0;
        if !t.trim().is_empty() && self.hist.last() != Some(&t) {
            self.hist.push(t.clone());
        }
        self.hist_idx = self.hist.len();
        t
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cur, s);
        self.cur += s.len();
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn word_left(&self) -> usize {
        let t = &self.text[..self.cur];
        let t = t.trim_end_matches(|c: char| !is_word(c));
        t.trim_end_matches(is_word).len()
    }

    fn word_right(&self) -> usize {
        let t = &self.text[self.cur..];
        let skip = t.len() - t.trim_start_matches(|c: char| !is_word(c)).len();
        let t = &t[skip..];
        self.cur + skip + (t.len() - t.trim_start_matches(is_word).len())
    }

    /// The word ending at the cursor (for completion), as (start, &str).
    pub fn word_before(&self, extra: &[char]) -> (usize, &str) {
        let t = &self.text[..self.cur];
        let s = t.trim_end_matches(|c: char| is_word(c) || c == '-' || c == '.' || extra.contains(&c)).len();
        (s, &self.text[s..self.cur])
    }

    pub fn replace(&mut self, start: usize, s: &str) {
        self.text.replace_range(start..self.cur, s);
        self.cur = start + s.len();
    }

    pub fn key(&mut self, k: &Key) -> Edit {
        match k {
            Key::Char(c) => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
            }
            Key::Paste(s) => self.insert(s),
            Key::Alt('\n') => self.insert("\n"),
            Key::Left | Key::Ctrl('b') => self.cur = self.prev(self.cur),
            Key::Right | Key::Ctrl('f') => self.cur = self.next(self.cur),
            Key::Home | Key::Ctrl('a') => self.cur = self.text[..self.cur].rfind('\n').map_or(0, |i| i + 1),
            Key::End | Key::Ctrl('e') => self.cur = self.text[self.cur..].find('\n').map_or(self.text.len(), |i| self.cur + i),
            Key::Alt('b') => self.cur = self.word_left(),
            Key::Alt('f') => self.cur = self.word_right(),
            Key::Backspace | Key::Ctrl('h') if self.cur > 0 => {
                let p = self.prev(self.cur);
                self.text.replace_range(p..self.cur, "");
                self.cur = p;
            }
            Key::Delete | Key::Ctrl('d') if self.cur < self.text.len() => {
                let n = self.next(self.cur);
                self.text.replace_range(self.cur..n, "");
            }
            Key::Ctrl('w') => {
                let p = self.word_left();
                self.text.replace_range(p..self.cur, "");
                self.cur = p;
            }
            Key::Alt('d') => {
                let n = self.word_right();
                self.text.replace_range(self.cur..n, "");
            }
            Key::Ctrl('u') => {
                let s = self.text[..self.cur].rfind('\n').map_or(0, |i| i + 1);
                self.text.replace_range(s..self.cur, "");
                self.cur = s;
            }
            Key::Ctrl('k') => {
                let e = self.text[self.cur..].find('\n').map_or(self.text.len(), |i| self.cur + i);
                self.text.replace_range(self.cur..e, "");
            }
            Key::Up if !self.text[..self.cur].contains('\n') && self.hist_idx > 0 => {
                self.hist_idx -= 1;
                let h = self.hist[self.hist_idx].clone();
                self.set(&h);
            }
            Key::Down if !self.text[self.cur..].contains('\n') && self.hist_idx < self.hist.len() => {
                self.hist_idx += 1;
                let h = self.hist.get(self.hist_idx).cloned().unwrap_or_default();
                self.set(&h);
            }
            Key::Backspace | Key::Ctrl('h') | Key::Delete | Key::Ctrl('d') | Key::Up | Key::Down => {}
            _ => return Edit::Ignored,
        }
        Edit::Handled
    }

    /// Visual lines needed to show the text wrapped at `width` (after `prefix_w`).
    pub fn height(&self, width: usize, prefix_w: usize) -> usize {
        wrap(&self.text, width.saturating_sub(prefix_w).max(1)).len()
    }

    /// The byte offset under a click at (`col`, `row`) relative to where
    /// `draw` was called with the same `w` and `rows`.
    pub fn offset_at(&self, w: usize, rows: usize, col: usize, row: usize) -> usize {
        let lines = wrap(&self.text, w.max(1));
        let cl = lines.iter().rposition(|&(a, _)| a <= self.cur).unwrap_or(0);
        let top = (cl + 1).saturating_sub(rows);
        let Some(&(a, b)) = lines.get(top + row).or(lines.last()) else { return 0 };
        let mut x = 0;
        for (i, c) in self.text[a..b].char_indices() {
            let cw = crate::screen::char_width(c);
            if x + cw > col {
                return a + i;
            }
            x += cw;
        }
        b
    }

    /// Draw wrapped into rows `y..y+rows` (showing the cursor's part if it
    /// doesn't fit). Returns the cursor's screen position.
    pub fn draw(&self, scr: &mut Screen, x: usize, y: usize, w: usize, rows: usize, st: Style) -> (usize, usize) {
        let lines = wrap(&self.text, w.max(1));
        let cl = lines.iter().rposition(|&(a, _)| a <= self.cur).unwrap_or(0);
        let top = (cl + 1).saturating_sub(rows);
        let mut cpos = (x, y);
        for (r, &(a, b)) in lines.iter().enumerate().skip(top).take(rows) {
            let sy = y + r - top;
            scr.puts(x, sy, &self.text[a..b], st, x + w);
            if r == cl {
                cpos = (x + str_width(&self.text[a..self.cur.min(b)]), sy);
            }
        }
        cpos
    }
}
