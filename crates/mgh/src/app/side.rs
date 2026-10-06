// A file-tree sidebar shared by the review view (changed files) and the code
// browser (the whole repo).

use super::{ACCENT, FG_DIM};
use mtui::screen::{Screen, Style, BOLD};

const BG_SIDE: u8 = 234;
const BG_SEL: u8 = 237;
const BG_FOCUS: u8 = 24;

pub struct SRow {
    pub depth: usize,
    pub text: String,
    pub dir: bool,
    /// For directories in the repo browser: expanded or not.
    pub open: bool,
    /// A status letter and its color, drawn before a file name.
    pub mark: Option<(char, u8)>,
    /// Dim text at the right edge.
    pub extra: String,
    pub dim: bool,
    /// What a click or Enter on the row refers to (caller's index), if anything.
    pub target: Option<usize>,
}

/// Scroll state of a sidebar. The selection is only forced into view when it
/// changed, so wheel scrolling doesn't snap back to it.
#[derive(Default)]
pub struct SideState {
    pub top: usize,
    seen: Option<usize>,
}

/// Where the last draw put the sidebar, for mouse hits.
#[derive(Default, Clone, Copy)]
pub struct SideGeo {
    pub w: usize,
    pub y0: usize,
    pub top: usize,
}

/// Rows for a list of changed files (already sorted by path): a header per
/// run of new directories, then the files.
pub fn changed_rows(files: &[(String, char, u8, String, bool)]) -> Vec<SRow> {
    let mut out = Vec::new();
    let mut prev: Vec<&str> = Vec::new();
    for (i, (path, mark, color, extra, dim)) in files.iter().enumerate() {
        let parts: Vec<&str> = path.split('/').collect();
        let dirs = &parts[..parts.len() - 1];
        let common = dirs.iter().zip(prev.iter()).take_while(|(a, b)| a == b).count();
        if common < dirs.len() {
            out.push(SRow { depth: common, text: format!("{}/", dirs[common..].join("/")), dir: true, open: true, mark: None, extra: String::new(), dim: true, target: None });
        }
        out.push(SRow { depth: dirs.len(), text: parts[parts.len() - 1].to_string(), dir: false, open: false, mark: Some((*mark, *color)), extra: extra.clone(), dim: *dim, target: Some(i) });
        prev = dirs.to_vec();
    }
    out
}

/// Draw the rows into columns x..x+w (plus a divider at x+w) over rows
/// y0..y0+h, keeping `sel` (a row index) in view.
pub fn draw(scr: &mut Screen, rows: &[SRow], sel: Option<usize>, st: &mut SideState, (x, w): (usize, usize), (y0, h): (usize, usize), focused: bool) -> SideGeo {
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
    let top = &st.top;
    for y in y0..y0 + h {
        scr.fill(x, x + w, y, Style::new(0, BG_SIDE, 0));
        scr.put(x + w, y, '│', Style::fg(237));
    }
    for (k, r) in rows.iter().enumerate().skip(*top).take(h) {
        let y = y0 + k - *top;
        let selected = Some(k) == sel;
        let bg = if selected { if focused { BG_FOCUS } else { BG_SEL } } else { BG_SIDE };
        scr.fill(x, x + w, y, Style::new(0, bg, 0));
        if selected {
            scr.put(x, y, '▌', Style::new(ACCENT, bg, 0));
        }
        let mut cx = x + 1 + (r.depth * 2).min(w / 2);
        let fg = if r.dim { FG_DIM } else if selected { 255 } else { 250 };
        let st = Style::new(fg, bg, if selected || r.dir { BOLD } else { 0 });
        if r.dir {
            let arrow = if r.open { "▾ " } else { "▸ " };
            cx = scr.puts(cx, y, arrow, Style::new(FG_DIM, bg, 0), x + w);
        } else if let Some((c, col)) = r.mark {
            cx = scr.puts(cx, y, &format!("{} ", c), Style::new(col, bg, BOLD), x + w);
        }
        let room = (x + w).saturating_sub(cx + r.extra.chars().count() + 1);
        scr.puts(cx, y, &r.text, st, cx + room.max(4));
        if !r.extra.is_empty() {
            let ex = (x + w).saturating_sub(r.extra.chars().count() + 1);
            scr.puts(ex, y, &r.extra, Style::new(FG_DIM, bg, 0), x + w);
        }
    }
    SideGeo { w, y0, top: *top }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_dirs() {
        let f = |p: &str| (p.to_string(), 'M', 0u8, String::new(), false);
        let rows = changed_rows(&[f("README.md"), f("src/a.rs"), f("src/b.rs"), f("src/util/x.rs"), f("tests/t.rs")]);
        let s: Vec<String> = rows.iter().map(|r| format!("{}{}{}", " ".repeat(r.depth), r.text, r.target.map_or(String::new(), |t| format!("#{}", t)))).collect();
        assert_eq!(s, vec!["README.md#0", "src/", " a.rs#1", " b.rs#2", " util/", "  x.rs#3", "tests/", " t.rs#4"]);
    }
}
