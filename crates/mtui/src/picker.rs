// A fuzzy picker: a query prompt over a filtered, scrollable list, drawn as a
// bottom-docked overlay.

use crate::fuzzy;
use crate::screen::{Screen, Style, BOLD};
use crate::term::{Key, Mouse, MouseKind};

pub const FG_DIM: u8 = 242;
pub const BG_STATUS: u8 = 236;
pub const BG_LIST: u8 = 234;
pub const BG_SEL: u8 = 24;
pub const ACCENT: u8 = 180;

pub trait Label {
    fn label(&self) -> &str;
}

impl Label for String {
    fn label(&self) -> &str {
        self
    }
}

pub struct Picker<T> {
    pub title: String,
    pub items: Vec<T>,
    pub filtered: Vec<usize>,
    pub query: String,
    pub sel: usize,
    pub scroll: usize,
    /// Where the last draw put the prompt row and how many list rows.
    y0: usize,
    rows: usize,
}

pub enum Pick {
    Continue,
    Cancel,
    Accept,
}

impl<T: Label> Picker<T> {
    pub fn new(title: &str, items: Vec<T>) -> Self {
        let mut p = Picker { title: title.into(), items, filtered: Vec::new(), query: String::new(), sel: 0, scroll: 0, y0: 0, rows: 0 };
        p.refilter();
        p
    }

    pub fn refilter(&mut self) {
        self.filtered = fuzzy::filter(&self.query, self.items.iter().map(|it| it.label()));
        self.sel = 0;
        self.scroll = 0;
    }

    pub fn selected(&self) -> Option<&T> {
        self.filtered.get(self.sel).map(|&i| &self.items[i])
    }

    pub fn move_sel(&mut self, d: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as isize;
        self.sel = ((self.sel as isize + d).rem_euclid(n)) as usize;
    }

    pub fn key(&mut self, k: &Key) -> Pick {
        match k {
            Key::Esc | Key::Ctrl('c') => return Pick::Cancel,
            Key::Enter => return Pick::Accept,
            Key::Up | Key::Ctrl('p') | Key::Ctrl('k') | Key::BackTab => self.move_sel(-1),
            Key::Down | Key::Ctrl('n') | Key::Ctrl('j') | Key::Tab => self.move_sel(1),
            Key::PageUp => self.move_sel(-10),
            Key::PageDown => self.move_sel(10),
            Key::Backspace => {
                if self.query.pop().is_none() {
                    return Pick::Cancel;
                }
                self.refilter();
            }
            Key::Ctrl('u') => {
                self.query.clear();
                self.refilter();
            }
            Key::Char(c) => {
                self.query.push(*c);
                self.refilter();
            }
            Key::Paste(s) => {
                self.query.push_str(s.lines().next().unwrap_or(""));
                self.refilter();
            }
            Key::Mouse(m) => return self.mouse(m),
            _ => {}
        }
        Pick::Continue
    }

    /// Wheel moves the selection, clicking an item picks it, clicking
    /// above the picker cancels.
    fn mouse(&mut self, m: &Mouse) -> Pick {
        match m.kind {
            MouseKind::WheelUp => self.move_sel(-1),
            MouseKind::WheelDown => self.move_sel(1),
            MouseKind::Press(0) if m.y > self.y0 && m.y <= self.y0 + self.rows => {
                let i = self.scroll + m.y - self.y0 - 1;
                if i < self.filtered.len() {
                    self.sel = i;
                    return Pick::Accept;
                }
            }
            MouseKind::Press(_) if m.y < self.y0 => return Pick::Cancel,
            _ => {}
        }
        Pick::Continue
    }

    /// Draw the prompt and list so the last list row sits just above `bottom`.
    /// Returns the cursor position in the prompt.
    pub fn draw(&mut self, scr: &mut Screen, bottom: usize) -> (usize, usize) {
        let w = scr.w;
        let list_h = (scr.h / 2).max(5).min(bottom.saturating_sub(1)).min(self.filtered.len().max(1));
        let y_prompt = bottom.saturating_sub(list_h + 1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        }
        if self.sel >= self.scroll + list_h {
            self.scroll = self.sel + 1 - list_h;
        }
        (self.y0, self.rows) = (y_prompt, list_h);
        scr.fill(0, w, y_prompt, Style::new(252, BG_STATUS, 0));
        let x = scr.puts(0, y_prompt, &format!(" {} ", self.title), Style::new(16, ACCENT, BOLD), w);
        let x = scr.puts(x + 1, y_prompt, "> ", Style::new(ACCENT, BG_STATUS, BOLD), w);
        let x2 = scr.puts(x, y_prompt, &self.query, Style::new(255, BG_STATUS, 0), w);
        let cnt = format!("{}/{} ", self.filtered.len(), self.items.len());
        scr.puts(w.saturating_sub(cnt.len()), y_prompt, &cnt, Style::new(FG_DIM, BG_STATUS, 0), w);
        for k in 0..list_h {
            let y = y_prompt + 1 + k;
            let idx = self.scroll + k;
            let selected = idx == self.sel;
            let bg = if selected { BG_SEL } else { BG_LIST };
            scr.fill(0, w, y, Style::new(0, bg, 0));
            if let Some(&ii) = self.filtered.get(idx) {
                let st = Style::new(if selected { 255 } else { 250 }, bg, if selected { BOLD } else { 0 });
                scr.puts(1, y, if selected { "▌" } else { " " }, Style::new(ACCENT, bg, 0), w);
                scr.puts(3, y, self.items[ii].label(), st, w);
            }
        }
        (x2, y_prompt)
    }
}
