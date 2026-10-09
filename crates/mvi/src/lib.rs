//! mvi as a library: the editor itself (`Editor`), a ready-made event loop
//! (`run`) and a helper that lets another full-screen program hand a file to
//! it and take the result back (`edit_file`).

pub mod buffer;
pub mod complete;
pub mod diag;
pub mod editor;
pub mod ex;
pub mod live;
pub mod picker;
pub mod preview;
pub mod render;

pub use editor::Editor;
use mtui::syntax;

use mtui::term;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Apply `~/.config/mvi/config` (one ex command per line) to the editor.
pub fn load_config(ed: &mut Editor) {
    let Some(home) = std::env::var_os("HOME") else { return };
    let Ok(s) = std::fs::read_to_string(PathBuf::from(home).join(".config/mvi/config")) else { return };
    for l in s.lines() {
        let l = l.trim();
        if !l.is_empty() && !l.starts_with('"') && !l.starts_with('#') {
            ed.ex(l);
        }
    }
    ed.msg.clear();
}

/// Run the editor on the terminal until it quits. The caller has put the
/// terminal in raw mode and owns `input`; mouse reporting is switched to
/// the editor's setting, so a caller that wants its own back must restore it.
pub fn run(ed: &mut Editor, input: &mut term::Input) {
    term::set_mouse(ed.opts.mouse);
    loop {
        ed.render();
        let timeout = if ed.check_running { 100 } else if ed.mmd_active() { 150 } else { 1000 };
        if let Some(k) = input.next_key(timeout) {
            ed.handle_key(k);
            // drain whatever is already buffered before redrawing (paste, fast typing, ssh bursts)
            let mut n = 0;
            while !ed.quit && n < 4096 && input.ready() {
                match input.next_key(0) {
                    Some(k) => ed.handle_key(k),
                    None => break,
                }
                n += 1;
            }
        }
        if term::RESIZED.swap(false, Ordering::Relaxed) {
            let (w, h) = term::size();
            ed.screen.resize(w, h);
        }
        ed.poll_check();
        ed.sync_mmd();
        if mtui::theme::poll() {
            ed.screen.invalidate();
        }
        if ed.suspend {
            ed.suspend = false;
            term::suspend();
            let (w, h) = term::size();
            ed.screen.resize(w, h);
        }
        if ed.quit {
            break;
        }
    }
}

/// Edit `path` in a full-screen editor session (with the user's mvi config),
/// starting on 1-based `line`. Returns when the editor quits; the caller reads
/// the file for the result and must redraw its own screen.
pub fn edit_file(input: &mut term::Input, path: &Path, line: usize) {
    let (w, h) = term::size();
    let mut ed = Editor::new(w, h);
    load_config(&mut ed);
    ed.open_file(path);
    ed.switch_buf(0);
    let y = line.saturating_sub(1).min(ed.bb().lines.len() - 1);
    ed.set_cursor((y, 0));
    run(&mut ed, input);
}

/// An editor living inside another program's screen: it draws into a
/// rectangle of the host's `Screen` and gets the keys the host passes it, so
/// the host's own view stays visible around it.
pub struct Pane {
    pub ed: Editor,
}

impl Pane {
    /// An editor `w` x `h` cells large on `path`, starting on 1-based `line`.
    pub fn open(w: usize, h: usize, path: &Path, line: usize) -> Pane {
        let mut ed = Editor::new(w, h);
        load_config(&mut ed);
        ed.open_file(path);
        ed.switch_buf(0);
        let y = line.saturating_sub(1).min(ed.bb().lines.len() - 1);
        ed.set_cursor((y, 0));
        Pane { ed }
    }

    /// Feed a key; mouse positions are relative to the pane's top-left.
    pub fn key(&mut self, k: term::Key) {
        self.ed.handle_key(k);
        self.ed.suspend = false;
        self.ed.poll_check();
        self.ed.sync_mmd();
    }

    /// The editor has quit (`:q`, `:wq`, ...).
    pub fn finished(&self) -> bool {
        self.ed.quit
    }

    /// Draw into `scr` with the pane's top-left at (x, y), resizing to
    /// `w` x `h`; returns the cursor position on `scr` (x, y, bar shape).
    pub fn draw(&mut self, scr: &mut mtui::screen::Screen, (x, y): (usize, usize), (w, h): (usize, usize)) -> Option<(usize, usize, bool)> {
        if (self.ed.screen.w, self.ed.screen.h) != (w, h) {
            self.ed.screen.resize(w, h);
        }
        let cur = self.ed.draw();
        scr.blit(&self.ed.screen, x, y);
        cur.map(|(cx, cy, bar)| (cx + x, cy + y, bar))
    }
}
