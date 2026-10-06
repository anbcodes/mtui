// :preview — the buffer rendered as markdown in a full-screen pager.

use crate::editor::Editor;
use mtui::markdown::{self, Line, Opts};
use mtui::screen::{Style, BOLD};
use mtui::term::{Key, Mouse, MouseKind};

pub struct Preview {
    pub lines: Vec<Line>,
    /// Width the lines were wrapped for; a resize re-renders.
    width: usize,
    pub top: usize,
    /// First source line showing, so a re-render keeps the place.
    anchor: usize,
}

impl Editor {
    fn render_md(&self, width: usize) -> Vec<Line> {
        let text = self.bb().lines.join("\n");
        markdown::render(&text, &Opts { width: width.saturating_sub(2), images: false, hard_breaks: false })
    }

    pub fn open_preview(&mut self) {
        let w = self.screen.w;
        let lines = self.render_md(w);
        let cy = self.bb().cy;
        let top = lines.iter().position(|l| l.src >= cy).unwrap_or(lines.len().saturating_sub(1));
        self.preview = Some(Preview { anchor: lines.get(top).map_or(0, |l| l.src), lines, width: w, top });
    }

    fn view_h(&self) -> usize {
        self.screen.h.saturating_sub(1).max(1)
    }

    fn scroll_preview(&mut self, d: isize) {
        let vh = self.view_h();
        let Some(p) = &mut self.preview else { return };
        p.top = (p.top as isize + d).clamp(0, p.lines.len().saturating_sub(vh) as isize) as usize;
        p.anchor = p.lines.get(p.top).map_or(0, |l| l.src);
    }

    pub fn preview_key(&mut self, k: Key) {
        let page = self.view_h() as isize;
        match k {
            Key::Char('q') | Key::Esc | Key::Ctrl('c') => {
                self.preview = None;
                self.screen.invalidate();
            }
            Key::Enter => {
                // back to the editor at the source of the top line
                if let Some(p) = self.preview.take() {
                    let line = p.lines.get(p.top).map_or(0, |l| l.src);
                    self.set_cursor((line, 0));
                }
                self.screen.invalidate();
            }
            Key::Char('j') | Key::Down | Key::Ctrl('n') => self.scroll_preview(1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => self.scroll_preview(-1),
            Key::Ctrl('d') => self.scroll_preview(page / 2),
            Key::Ctrl('u') => self.scroll_preview(-page / 2),
            Key::Char(' ') | Key::PageDown | Key::Ctrl('f') => self.scroll_preview(page),
            Key::PageUp | Key::Ctrl('b') => self.scroll_preview(-page),
            Key::Char('g') | Key::Home => self.scroll_preview(isize::MIN / 2),
            Key::Char('G') | Key::End => self.scroll_preview(isize::MAX / 2),
            Key::Char('r') => {
                let anchor = self.preview.as_ref().map_or(0, |p| p.anchor);
                self.set_cursor((anchor, 0));
                self.open_preview();
            }
            Key::Mouse(m) => self.preview_mouse(m),
            _ => {}
        }
    }

    pub fn preview_mouse(&mut self, m: Mouse) {
        match m.kind {
            MouseKind::WheelUp => self.scroll_preview(-3),
            MouseKind::WheelDown => self.scroll_preview(3),
            _ => {}
        }
    }

    pub fn render_preview(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        if self.preview.as_ref().is_some_and(|p| p.width != w) {
            let lines = self.render_md(w);
            let p = self.preview.as_mut().unwrap();
            let top = lines.iter().position(|l| l.src >= p.anchor).unwrap_or(0);
            (p.lines, p.width, p.top) = (lines, w, top);
        }
        self.scroll_preview(0);
        self.screen.clear();
        let vh = self.view_h();
        let p = self.preview.as_ref().unwrap();
        for (k, l) in p.lines.iter().enumerate().skip(p.top).take(vh) {
            let mut x = 1;
            for (t, st) in &l.spans {
                x = self.screen.puts(x, k - p.top, t, *st, w);
            }
        }
        let pct = if p.lines.len() <= vh { 100 } else { (p.top + vh).min(p.lines.len()) * 100 / p.lines.len() };
        let y = h - 1;
        let bar = Style::new(250, 236, 0);
        self.screen.fill(0, w, y, bar);
        let x = self.screen.puts(0, y, " PREVIEW ", Style::new(16, 110, BOLD), w);
        let name = self.bb().name.clone();
        self.screen.puts(x + 1, y, &format!("{}   q close · Enter go to source · j k C-d C-u g G", name), bar, w);
        let s = format!("{}% ", pct);
        self.screen.puts(w.saturating_sub(s.len()), y, &s, Style::new(242, 236, 0), w);
        self.screen.flush(None);
    }
}
