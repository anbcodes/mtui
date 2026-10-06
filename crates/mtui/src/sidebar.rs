// A sidebar: a scrollable column of rows (a file tree, a folder list, the
// items of the list you're reading) with one selected row, drawn beside the
// main view with a divider. The caller owns the rows and the selection; this
// module owns scrolling, drawing and mapping a mouse position back to a row.
//
// Typical use: build `Row`s each frame, call `draw` with a persistent `State`,
// keep the returned `Geo`, and ask it `row_at(x, y)` on a click.

use crate::picker::{ACCENT, FG_DIM};
use crate::screen::{Screen, Style, BOLD};

const BG_SIDE: u8 = 234;
const BG_SEL: u8 = 237;
const BG_FOCUS: u8 = 24;

#[derive(Clone, Default)]
pub struct Row {
    pub depth: usize,
    pub text: String,
    /// A directory-like row: drawn with an arrow and bold.
    pub dir: bool,
    /// For `dir` rows: expanded or not.
    pub open: bool,
    /// A letter or symbol and its color, drawn before the text.
    pub mark: Option<(char, u8)>,
    /// Text at the right edge (a count, a date) and its color (0 = dim).
    pub extra: String,
    pub extra_fg: u8,
    pub dim: bool,
    pub bold: bool,
    /// The row the main view is showing, as opposed to the one selected: it
    /// gets the accent bar even while another row is selected.
    pub current: bool,
    /// What a click or Enter on the row refers to (caller's index), if anything.
    pub target: Option<usize>,
}

impl Row {
    pub fn new(text: impl Into<String>) -> Row {
        Row { text: text.into(), ..Row::default() }
    }
}

/// Scroll state. The selection is only forced into view when it changed, so
/// wheel scrolling doesn't snap back to it.
#[derive(Default)]
pub struct State {
    pub top: usize,
    seen: Option<usize>,
}

impl State {
    /// Scroll by `d` rows (wheel); `draw` clamps.
    pub fn scroll(&mut self, d: isize) {
        self.top = (self.top as isize + d).max(0) as usize;
    }
}

/// Where the last draw put the sidebar, for mouse hits.
#[derive(Default, Clone, Copy)]
pub struct Geo {
    pub w: usize,
    pub y0: usize,
    pub top: usize,
}

impl Geo {
    pub fn contains(&self, x: usize) -> bool {
        self.w > 0 && x <= self.w
    }

    /// The row index (into the slice given to `draw`) under screen cell (x, y).
    pub fn row_at(&self, x: usize, y: usize) -> Option<usize> {
        (self.contains(x) && y >= self.y0).then(|| self.top + y - self.y0)
    }
}

/// Draw `rows` into columns x..x+w (plus a divider at x+w) over screen rows
/// y0..y0+h, keeping row `sel` in view and highlighted (brighter when `focused`).
pub fn draw(scr: &mut Screen, rows: &[Row], sel: Option<usize>, st: &mut State, (x, w): (usize, usize), (y0, h): (usize, usize), focused: bool) -> Geo {
    if sel != st.seen {
        st.seen = sel;
        if let Some(s) = sel {
            if s < st.top {
                st.top = s;
                // show the directory header above the first file too
                while st.top > 0 && rows[st.top - 1].dir {
                    st.top -= 1;
                }
            }
            if s >= st.top + h {
                st.top = s + 1 - h;
            }
        }
    }
    st.top = st.top.min(rows.len().saturating_sub(h));
    let top = st.top;
    for y in y0..y0 + h {
        scr.fill(x, x + w, y, Style::new(0, BG_SIDE, 0));
        scr.put(x + w, y, '│', Style::fg(237));
    }
    for (k, r) in rows.iter().enumerate().skip(top).take(h) {
        let y = y0 + k - top;
        let selected = Some(k) == sel;
        let bg = if selected { if focused { BG_FOCUS } else { BG_SEL } } else { BG_SIDE };
        scr.fill(x, x + w, y, Style::new(0, bg, 0));
        if selected || r.current {
            scr.put(x, y, '▌', Style::new(ACCENT, bg, 0));
        }
        let mut cx = x + 1 + (r.depth * 2).min(w / 2);
        let fg = if r.dim { FG_DIM } else if selected || r.current { 255 } else { 250 };
        let st = Style::new(fg, bg, if selected || r.dir || r.bold || r.current { BOLD } else { 0 });
        if r.dir {
            let arrow = if r.open { "▾ " } else { "▸ " };
            cx = scr.puts(cx, y, arrow, Style::new(FG_DIM, bg, 0), x + w);
        } else if let Some((c, col)) = r.mark {
            cx = scr.puts(cx, y, &format!("{} ", c), Style::new(col, bg, BOLD), x + w);
        }
        let ew = crate::wrap::str_width(&r.extra);
        let room = (x + w).saturating_sub(cx + ew + if ew > 0 { 2 } else { 1 });
        scr.puts(cx, y, &r.text, st, cx + room.max(4));
        if !r.extra.is_empty() {
            let ex = (x + w).saturating_sub(ew + 1);
            scr.puts(ex, y, &r.extra, Style::new(if r.extra_fg == 0 { FG_DIM } else { r.extra_fg }, bg, if r.extra_fg == 0 { 0 } else { BOLD }), x + w);
        }
    }
    Geo { w, y0, top }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_to_selection_only_when_it_changes() {
        let mut scr = Screen::new(30, 10);
        let rows: Vec<Row> = (0..20).map(|i| Row { target: Some(i), ..Row::new(format!("row {}", i)) }).collect();
        let mut st = State::default();
        let g = draw(&mut scr, &rows, Some(15), &mut st, (0, 10), (1, 5), true);
        assert_eq!((g.top, st.top), (11, 11));
        assert_eq!(g.row_at(3, 2), Some(12));
        assert_eq!(g.row_at(11, 2), None);
        st.scroll(-4);
        let g = draw(&mut scr, &rows, Some(15), &mut st, (0, 10), (1, 5), true);
        assert_eq!(g.top, 7, "wheel scroll isn't undone while the selection stays put");
        let g = draw(&mut scr, &rows, Some(2), &mut st, (0, 10), (1, 5), true);
        assert_eq!(g.top, 2);
    }
}
