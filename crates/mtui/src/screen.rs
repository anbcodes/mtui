// Double-buffered cell grid. Only changed cells are sent to the terminal,
// which keeps redraws cheap over slow links.

use crate::kitty::{self, Placement};
use std::io::Write;

pub const BOLD: u8 = 1;
pub const UNDERLINE: u8 = 2;
pub const REVERSE: u8 = 4;
pub const ITALIC: u8 = 8;
pub const UNDERCURL: u8 = 16;

/// fg/bg are xterm-256 indices; 0 means "terminal default".
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Style {
    pub fg: u8,
    pub bg: u8,
    pub attr: u8,
}

impl Style {
    pub const fn fg(fg: u8) -> Style {
        Style { fg, bg: 0, attr: 0 }
    }
    pub const fn new(fg: u8, bg: u8, attr: u8) -> Style {
        Style { fg, bg, attr }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char, // '\0' = continuation of a wide char
    pub st: Style,
}

const BLANK: Cell = Cell { ch: ' ', st: Style { fg: 0, bg: 0, attr: 0 } };

pub struct Screen {
    pub w: usize,
    pub h: usize,
    cells: Vec<Cell>,
    prev: Vec<Cell>,
    out: Vec<u8>,
    full: bool,
    images: Vec<Placement>,
    shown: Vec<Placement>,
}

pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x1100 {
        return 1;
    }
    let wide = (0x1100..=0x115f).contains(&u)
        || (0x2e80..=0x303e).contains(&u)
        || (0x3041..=0x33ff).contains(&u)
        || (0x3400..=0x4dbf).contains(&u)
        || (0x4e00..=0x9fff).contains(&u)
        || (0xa000..=0xa4cf).contains(&u)
        || (0xac00..=0xd7a3).contains(&u)
        || (0xf900..=0xfaff).contains(&u)
        || (0xfe30..=0xfe4f).contains(&u)
        || (0xff00..=0xff60).contains(&u)
        || (0xffe0..=0xffe6).contains(&u)
        || (0x1f300..=0x1f64f).contains(&u)
        || (0x1f900..=0x1f9ff).contains(&u)
        || (0x20000..=0x3fffd).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

impl Screen {
    pub fn new(w: usize, h: usize) -> Self {
        Screen { w, h, cells: vec![BLANK; w * h], prev: vec![BLANK; w * h], out: Vec::with_capacity(8192), full: true, images: Vec::new(), shown: Vec::new() }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.cells = vec![BLANK; w * h];
        self.prev = vec![BLANK; w * h];
        self.full = true;
    }

    /// Images to show over the cells in the next `flush`. Only changes are
    /// sent. Leave empty while an overlay is drawn: images sit above text.
    /// The kitty image id drawn over cell (x, y) in the current frame, for
    /// click-to-open.
    pub fn image_at(&self, x: usize, y: usize) -> Option<u32> {
        self.images.iter().find(|p| x >= p.x && x < p.x + p.cols && y >= p.y && y < p.y + p.rows).map(|p| p.id)
    }

    pub fn set_images(&mut self, v: Vec<Placement>) {
        self.images = v;
    }

    pub fn invalidate(&mut self) {
        self.full = true;
    }

    pub fn clear(&mut self) {
        self.cells.fill(BLANK);
    }

    /// Put one char; returns the number of columns used.
    pub fn put(&mut self, x: usize, y: usize, c: char, st: Style) -> usize {
        if x >= self.w || y >= self.h {
            return 0;
        }
        let c = if (c as u32) < 0x20 || c == '\x7f' { '?' } else { c };
        let cw = char_width(c);
        let i = y * self.w + x;
        if cw == 2 {
            if x + 1 >= self.w {
                self.cells[i] = Cell { ch: ' ', st };
                return 1;
            }
            self.cells[i] = Cell { ch: c, st };
            self.cells[i + 1] = Cell { ch: '\0', st };
            2
        } else {
            self.cells[i] = Cell { ch: c, st };
            1
        }
    }

    /// Put a string, clipped at `max_x`. Returns the x after the last char.
    pub fn puts(&mut self, mut x: usize, y: usize, s: &str, st: Style, max_x: usize) -> usize {
        let max_x = max_x.min(self.w);
        for c in s.chars() {
            if x >= max_x || x + char_width(c) > max_x {
                break;
            }
            x += self.put(x, y, c, st);
        }
        x
    }

    pub fn fill(&mut self, x0: usize, x1: usize, y: usize, st: Style) {
        for x in x0..x1.min(self.w) {
            self.put(x, y, ' ', st);
        }
    }

    pub fn set_style(&mut self, x: usize, y: usize, st: Style) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x].st = st;
        }
    }

    pub fn get_style(&self, x: usize, y: usize) -> Style {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x].st
        } else {
            Style::default()
        }
    }

    fn sgr(out: &mut Vec<u8>, st: Style) {
        out.extend_from_slice(b"\x1b[0");
        if st.attr & BOLD != 0 {
            out.extend_from_slice(b";1");
        }
        if st.attr & ITALIC != 0 {
            out.extend_from_slice(b";3");
        }
        if st.attr & UNDERCURL != 0 {
            out.extend_from_slice(b";4:3");
        } else if st.attr & UNDERLINE != 0 {
            out.extend_from_slice(b";4");
        }
        if st.attr & REVERSE != 0 {
            out.extend_from_slice(b";7");
        }
        if st.fg != 0 {
            let _ = write!(out, ";38;5;{}", st.fg);
        }
        if st.bg != 0 {
            let _ = write!(out, ";48;5;{}", st.bg);
        }
        out.push(b'm');
    }

    /// Emit the diff to the terminal. `cursor` = (x, y, bar_shape).
    pub fn flush(&mut self, cursor: Option<(usize, usize, bool)>) {
        let out = &mut self.out;
        out.clear();
        out.extend_from_slice(b"\x1b[?25l");
        if self.full {
            out.extend_from_slice(b"\x1b[0m\x1b[2J");
            self.prev.fill(Cell { ch: '\u{1}', st: Style::default() });
            // Clearing the screen also drops the terminal's placements.
            self.shown.clear();
        }
        let mut cur_st: Option<Style> = None;
        let (mut cx, mut cy) = (usize::MAX, usize::MAX);
        let w = self.w;
        for y in 0..self.h {
            let mut x = 0;
            while x < w {
                let i = y * w + x;
                if self.cells[i] == self.prev[i] {
                    x += 1;
                    continue;
                }
                let hx = if self.cells[i].ch == '\0' && x > 0 { x - 1 } else { x };
                let cell = self.cells[y * w + hx];
                if cell.ch == '\0' {
                    // orphan continuation; draw a space
                    self.cells[y * w + hx] = Cell { ch: ' ', st: cell.st };
                }
                let cell = self.cells[y * w + hx];
                if cy != y || cx != hx {
                    let _ = write!(out, "\x1b[{};{}H", y + 1, hx + 1);
                }
                if cur_st != Some(cell.st) {
                    Self::sgr(out, cell.st);
                    cur_st = Some(cell.st);
                }
                let mut b = [0u8; 4];
                out.extend_from_slice(cell.ch.encode_utf8(&mut b).as_bytes());
                let cw = if hx + 1 < w && self.cells[y * w + hx + 1].ch == '\0' { 2 } else { 1 };
                cx = hx + cw;
                cy = y;
                x = hx + cw;
            }
        }
        out.extend_from_slice(b"\x1b[0m");
        for p in self.shown.iter().filter(|p| !self.images.iter().any(|q| q.id == p.id && q.pid == p.pid)) {
            out.extend_from_slice(&kitty::unplace(p));
        }
        for p in self.images.iter().filter(|p| !self.shown.contains(p)) {
            let _ = write!(out, "\x1b[{};{}H", p.y + 1, p.x + 1);
            out.extend_from_slice(&kitty::place(p));
        }
        self.shown.clone_from(&self.images);
        if let Some((x, y, bar)) = cursor {
            let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
            out.extend_from_slice(if bar { b"\x1b[6 q" } else { b"\x1b[2 q" });
            out.extend_from_slice(b"\x1b[?25h");
        }
        std::mem::swap(&mut self.cells, &mut self.prev);
        self.cells.copy_from_slice(&self.prev);
        self.full = false;
        let mut so = std::io::stdout().lock();
        let _ = so.write_all(out);
        let _ = so.flush();
    }

    /// Copy to the system clipboard via OSC 52 (works over ssh).
    pub fn osc52(&self, s: &str) {
        self.raw(format!("\x1b]52;c;{}\x07", crate::base64::encode(s.as_bytes())).as_bytes());
    }

    /// Raw passthrough write (e.g. OSC 52 clipboard).
    pub fn raw(&self, s: &[u8]) {
        let mut so = std::io::stdout().lock();
        let _ = so.write_all(s);
        let _ = so.flush();
    }
}
