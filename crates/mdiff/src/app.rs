// The viewer: changed files down the side, the selected file's diff with
// syntax highlighting and the lines added by the latest change marked, the
// newest entry of an explanation log, and an mvi pane to edit the file at
// the cursor while the diff above keeps updating.

use crate::diff::{self, File, RK};
use mtui::screen::{Screen, Style, BOLD, ITALIC};
use mtui::sidebar;
use mtui::syntax::{self, State};
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::wrap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const TEXT: u8 = 252;
const DIM: u8 = 246;
const ACCENT: u8 = 180;
const OK: u8 = 114;
const BAD: u8 = 203;
const WARN: u8 = 209;
const HUNK: u8 = 73;
const FRESH: u8 = 220;
const BAR: u8 = 236;
const BG_ADD: u8 = 22;
const BG_DEL: u8 = 52;
const BG_ADD_CUR: u8 = 28;
const BG_DEL_CUR: u8 = 88;
const BG_CUR: u8 = 238;
const BG_NOTE: u8 = 235;

const HELP: &[&str] = &[
    "j k  ↓ ↑       move          Ctrl-D Ctrl-U  half a page   g G  top, bottom",
    "J K  Tab       next / previous file          n N  next / previous hunk",
    "← →            scroll sideways",
    "Enter e        edit the file at this line in an mvi pane below the diff",
    "Ctrl-W         switch between the diff and the pane (from normal mode)",
    "               :w saves and the diff updates; :q closes the pane",
    "f              follow: jump to whatever changes next (on at start)",
    "x              show / hide the latest explanation entry",
    "s              show / hide the file list",
    "q              quit",
    "",
    "▎ in the gutter marks lines added by the latest change.",
];

pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub suspend: bool,
    root: PathBuf,
    base: String,
    files: Vec<File>,
    /// Per file, per row: added by the change that last touched the file.
    fresh: Vec<Vec<bool>>,
    loaded: bool,
    cur: usize,
    cursor: usize,
    top: usize,
    /// Bring the cursor into view on the next draw (false after wheel scrolls).
    keep: bool,
    hscroll: usize,
    hl: Vec<Vec<u8>>,
    hl_key: Option<(usize, u64)>,
    ver: u64,
    follow: bool,
    side: bool,
    side_st: sidebar::State,
    geo: sidebar::Geo,
    /// x, y, w, h of the diff rows and of the pane, from the last draw.
    diff_at: (usize, usize, usize, usize),
    pane_at: (usize, usize, usize, usize),
    notes: Option<PathBuf>,
    /// The log's path in the tree, kept out of the file list.
    notes_rel: Option<String>,
    notes_stamp: Option<SystemTime>,
    entry: Vec<String>,
    show_entry: bool,
    pane: Option<mvi::Pane>,
    pane_focus: bool,
    help: bool,
    msg: String,
    err: String,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The newest entry of an explanation log: its first `## ` section (the log
/// is newest first). Nothing until there is one.
fn newest_entry(s: &str) -> Vec<String> {
    let lines: Vec<&str> = s.lines().collect();
    let Some(a) = lines.iter().position(|l| l.starts_with("## ")) else { return vec![] };
    let b = lines[a + 1..].iter().position(|l| l.starts_with("## ")).map_or(lines.len(), |b| a + 1 + b);
    let mut v: Vec<String> = lines[a..b].iter().map(|l| l.to_string()).collect();
    while v.last().is_some_and(|l| l.trim().is_empty()) {
        v.pop();
    }
    v
}

fn ago(t: SystemTime) -> String {
    let s = t.elapsed().map_or(0, |d| d.as_secs());
    match s {
        0..=59 => format!("{}s ago", s),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

/// The first row that differs between two versions of a file.
fn first_change(old: &File, new: &File) -> usize {
    (0..new.rows.len()).find(|&k| old.rows.get(k) != Some(&new.rows[k])).unwrap_or(new.rows.len().saturating_sub(1))
}

impl App {
    pub fn new(w: usize, h: usize, root: PathBuf, base: String, notes: Option<PathBuf>) -> App {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        // the log may not exist yet: resolve its directory instead
        let notes_rel = notes.as_ref().and_then(|n| {
            let dir = canon(n.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")));
            let full = dir.join(n.file_name()?);
            full.strip_prefix(canon(&root)).ok().map(|r| r.to_string_lossy().into_owned())
        });
        App {
            screen: Screen::new(w, h),
            quit: false,
            suspend: false,
            root,
            base,
            files: vec![],
            fresh: vec![],
            loaded: false,
            cur: 0,
            cursor: 0,
            top: 0,
            keep: true,
            hscroll: 0,
            hl: vec![],
            hl_key: None,
            ver: 0,
            follow: true,
            side: true,
            side_st: sidebar::State::default(),
            geo: sidebar::Geo::default(),
            diff_at: (0, 0, 0, 0),
            pane_at: (0, 0, 0, 0),
            notes,
            notes_rel,
            notes_stamp: None,
            entry: vec![],
            show_entry: true,
            pane: None,
            pane_focus: false,
            help: false,
            msg: String::new(),
            err: String::new(),
        }
    }

    /// A new snapshot from the watcher.
    pub fn on_snapshot(&mut self, r: Result<Vec<File>, String>) {
        let mut files = match r {
            Ok(f) => f,
            Err(e) => {
                self.err = e;
                return;
            }
        };
        self.err.clear();
        if let Some(n) = &self.notes_rel {
            files.retain(|f| &f.name != n);
        }
        let first = !self.loaded;
        self.loaded = true;
        let mut fresh = Vec::with_capacity(files.len());
        let mut changed: Option<(usize, usize)> = None;
        for (i, f) in files.iter().enumerate() {
            let old = self.files.iter().position(|o| o.name == f.name);
            let same = old.is_some_and(|o| self.files[o].rows == f.rows);
            let flags: Vec<bool> = match old {
                Some(o) if same => self.fresh[o].clone(),
                Some(o) => {
                    let seen: HashSet<&str> = self.files[o].rows.iter().filter(|r| r.kind == RK::Add).map(|r| r.text.as_str()).collect();
                    f.rows.iter().map(|r| r.kind == RK::Add && !seen.contains(r.text.as_str())).collect()
                }
                None if first => vec![false; f.rows.len()],
                None => f.rows.iter().map(|r| r.kind == RK::Add).collect(),
            };
            if !first && !same && changed.is_none() {
                let at = flags.iter().position(|&b| b).unwrap_or_else(|| old.map_or(0, |o| first_change(&self.files[o], f)));
                changed = Some((i, at));
            }
            fresh.push(flags);
        }
        let name = self.files.get(self.cur).map(|f| f.name.clone());
        self.files = files;
        self.fresh = fresh;
        self.ver += 1;
        match changed {
            Some((i, at)) if self.follow && !self.pane_focus => {
                self.cur = i;
                self.cursor = at;
                // put the change a third of the way down
                self.top = at.saturating_sub(self.diff_at.3 / 3);
                self.keep = true;
            }
            _ => {
                if let Some(i) = name.and_then(|n| self.files.iter().position(|f| f.name == n)) {
                    self.cur = i;
                }
            }
        }
        self.cur = self.cur.min(self.files.len().saturating_sub(1));
        self.cursor = self.cursor.min(self.rows_len().saturating_sub(1));
        self.reload_clean_buffers();
    }

    /// Files the editor has open but not modified follow changes made on disk
    /// (by the other session), so `:w` never writes back a stale copy.
    fn reload_clean_buffers(&mut self) {
        let Some(p) = &mut self.pane else { return };
        let ed = &mut p.ed;
        let mut clash = vec![];
        for b in ed.bufs.iter_mut() {
            let Some(path) = b.path.clone() else { continue };
            let Ok(s) = std::fs::read_to_string(&path) else { continue };
            let mut lines: Vec<&str> = s.lines().collect();
            if lines.is_empty() {
                lines.push("");
            }
            if lines.len() == b.lines.len() && lines.iter().zip(&b.lines).all(|(a, b)| a == b) {
                continue;
            }
            if b.dirty {
                clash.push(b.name.clone());
                continue;
            }
            let (cy, cx, top) = (b.cy, b.cx, b.top);
            *b = mvi::buffer::Buffer::new(Some(path));
            b.cy = cy.min(b.lines.len().saturating_sub(1));
            b.cx = if b.cy == cy { cx.min(b.lines[b.cy].len()) } else { 0 };
            b.top = top.min(b.cy);
        }
        ed.clamp();
        ed.screen.invalidate();
        if !clash.is_empty() {
            self.msg = format!("{} changed on disk under your unsaved edits: :w overwrites, :e! takes theirs", clash.join(", "));
        }
    }

    /// Reload the explanation log when it changes.
    pub fn tick(&mut self) {
        let Some(p) = &self.notes else { return };
        let m = mtime(p);
        if m != self.notes_stamp {
            self.notes_stamp = m;
            self.entry = std::fs::read_to_string(p).map(|s| newest_entry(&s)).unwrap_or_default();
        }
    }

    fn file(&self) -> Option<&File> {
        self.files.get(self.cur)
    }

    fn rows_len(&self) -> usize {
        self.file().map_or(0, |f| f.rows.len())
    }

    fn select(&mut self, i: usize) {
        if i != self.cur && i < self.files.len() {
            self.cur = i;
            self.cursor = 0;
            self.top = 0;
            self.hscroll = 0;
            self.keep = true;
        }
    }

    fn move_to(&mut self, i: usize) {
        self.cursor = i.min(self.rows_len().saturating_sub(1));
        self.keep = true;
    }

    fn hunk(&mut self, fwd: bool) {
        let Some(f) = self.file() else { return };
        let is = |k: usize| f.rows[k].kind == RK::Hunk;
        let to = if fwd { (self.cursor + 1..f.rows.len()).find(|&k| is(k)) } else { (0..self.cursor).rev().find(|&k| is(k)) };
        match to {
            Some(k) => {
                self.cursor = k;
                self.top = k; // the whole hunk below the header
            }
            None => self.msg = format!("no {} hunk", if fwd { "next" } else { "previous" }),
        }
    }

    // ---- editing ----

    fn edit(&mut self) {
        let Some(f) = self.file() else { return };
        if f.status == 'D' {
            self.msg = "the file is deleted: nothing to edit".into();
            return;
        }
        let line = diff::edit_line(&f.rows, self.cursor);
        let path = self.root.join(&f.name);
        match &mut self.pane {
            Some(p) => {
                p.ed.open_file(&path);
                let y = (line - 1).min(p.ed.bb().lines.len() - 1);
                p.ed.set_cursor((y, 0));
            }
            None => self.pane = Some(mvi::Pane::open(80, 20, &path, line)),
        }
        self.pane_focus = true;
    }

    fn pane_key(&mut self, k: Key) {
        let Some(p) = &mut self.pane else { return };
        if k == Key::Ctrl('w') && p.ed.mode == mvi::editor::Mode::Normal {
            self.pane_focus = false;
            return;
        }
        let mut k = k;
        if let Key::Mouse(m) = &mut k {
            let (x, y, w, h) = self.pane_at;
            if m.x < x || m.y < y || m.x >= x + w || m.y >= y + h {
                self.pane_focus = false;
                return self.mouse(*m);
            }
            m.x -= x;
            m.y -= y;
        }
        p.key(k);
        if p.finished() {
            self.pane = None;
            self.pane_focus = false;
        }
    }

    // ---- keys ----

    pub fn handle_key(&mut self, k: Key) {
        self.msg.clear();
        if self.pane_focus {
            return self.pane_key(k);
        }
        if self.help {
            self.help = false;
            return;
        }
        let n = self.rows_len();
        let page = self.diff_at.3.max(2);
        match k {
            Key::Char('q') | Key::Ctrl('c') => self.quit = true,
            Key::Ctrl('z') => self.suspend = true,
            Key::Char('?') => self.help = true,
            Key::Char('j') | Key::Down => self.move_to(self.cursor + 1),
            Key::Char('k') | Key::Up => self.move_to(self.cursor.saturating_sub(1)),
            Key::Ctrl('d') | Key::PageDown => {
                self.top = (self.top + page / 2).min(n.saturating_sub(1));
                self.move_to(self.cursor + page / 2);
            }
            Key::Ctrl('u') | Key::PageUp => {
                self.top = self.top.saturating_sub(page / 2);
                self.move_to(self.cursor.saturating_sub(page / 2));
            }
            Key::Char('g') | Key::Home => self.move_to(0),
            Key::Char('G') | Key::End => self.move_to(n),
            Key::Char('J') | Key::Tab => self.select((self.cur + 1).min(self.files.len().saturating_sub(1))),
            Key::Char('K') | Key::BackTab => self.select(self.cur.saturating_sub(1)),
            Key::Char('n') => self.hunk(true),
            Key::Char('N') => self.hunk(false),
            Key::Left => self.hscroll = self.hscroll.saturating_sub(8),
            Key::Right => self.hscroll += 8,
            Key::Enter | Key::Char('e') => self.edit(),
            Key::Ctrl('w') if self.pane.is_some() => self.pane_focus = true,
            Key::Char('f') => {
                self.follow = !self.follow;
                self.msg = if self.follow { "follow on: jumps to the next change" } else { "follow off" }.into();
            }
            Key::Char('x') => self.show_entry = !self.show_entry,
            Key::Char('s') => self.side = !self.side,
            Key::Mouse(m) => self.mouse(m),
            _ => {}
        }
    }

    fn mouse(&mut self, m: Mouse) {
        let (_, py, _, ph) = self.pane_at;
        if self.pane.is_some() && m.y >= py && m.y < py + ph && !self.geo.contains(m.x) {
            self.pane_focus = true;
            return self.pane_key(Key::Mouse(m));
        }
        let side = self.geo.contains(m.x);
        match m.kind {
            MouseKind::WheelUp if side => self.side_st.scroll(-3),
            MouseKind::WheelDown if side => self.side_st.scroll(3),
            MouseKind::WheelUp => {
                self.top = self.top.saturating_sub(3);
                self.keep = false;
            }
            MouseKind::WheelDown => {
                self.top = (self.top + 3).min(self.rows_len().saturating_sub(1));
                self.keep = false;
            }
            MouseKind::Press(0) if side => {
                if let Some(i) = self.geo.row_at(m.x, m.y) {
                    self.select(i);
                }
            }
            MouseKind::Press(0) => {
                let (_, y0, _, h) = self.diff_at;
                if m.y >= y0 && m.y < y0 + h {
                    self.cursor = (self.top + m.y - y0).min(self.rows_len().saturating_sub(1));
                }
            }
            _ => {}
        }
    }

    // ---- drawing ----

    fn ensure_hl(&mut self) {
        if self.hl_key == Some((self.cur, self.ver)) {
            return;
        }
        self.hl_key = Some((self.cur, self.ver));
        self.hl.clear();
        let Some(f) = self.files.get(self.cur) else { return };
        let first = f.rows.iter().find(|r| matches!(r.kind, RK::Add | RK::Ctx)).map_or("", |r| r.text.as_str());
        let lang = syntax::detect(Path::new(&f.name), first);
        // the new and old sides keep separate state through a hunk
        let (mut sn, mut so) = (State::Normal, State::Normal);
        for r in &f.rows {
            let mut hl = Vec::new();
            match r.kind {
                RK::Hunk => (sn, so) = (State::Normal, State::Normal),
                RK::Add => sn = syntax::highlight(lang, &r.text, sn, &mut hl),
                RK::Del => so = syntax::highlight(lang, &r.text, so, &mut hl),
                RK::Ctx => {
                    sn = syntax::highlight(lang, &r.text, sn, &mut hl);
                    so = sn;
                }
                RK::Note => {}
            }
            self.hl.push(hl);
        }
    }

    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if h < 4 || w < 20 {
            self.screen.flush(None);
            return;
        }
        let bar = Style::new(TEXT, BAR, 0);

        // title
        self.screen.fill(0, w, 0, bar);
        let name = self.root.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let x = self.screen.puts(1, 0, &format!("mdiff {}", name), Style::new(ACCENT, BAR, BOLD), w);
        self.screen.puts(x + 1, 0, &format!("since {}", self.base), Style::new(DIM, BAR, 0), w);
        let (a, d) = self.files.iter().fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
        let nf = self.files.len();
        let right = format!("{} file{}  +{} -{}{}", nf, if nf == 1 { "" } else { "s" }, a, d, if self.follow { "  follow" } else { "" });
        self.screen.puts(w.saturating_sub(right.chars().count() + 1), 0, &right, Style::new(TEXT, BAR, 0), w);

        // layout: file list | diff, then the explanation, then the pane
        let (y0, body) = (1, h - 2);
        let sw = if self.side && w >= 70 {
            let longest = self.files.iter().map(|f| f.name.chars().count()).max().unwrap_or(10);
            (longest + 12).clamp(18, w / 3)
        } else {
            0
        };
        let x0 = if sw > 0 { sw + 1 } else { 0 };
        let dw = w - x0;
        let entry: Vec<String> = if self.show_entry && !self.entry.is_empty() {
            let text = self.entry.join("\n");
            wrap(&text, dw.saturating_sub(3)).into_iter().map(|(a, b)| text[a..b].to_string()).take(8).collect()
        } else {
            vec![]
        };
        let eh = if entry.is_empty() { 0 } else { entry.len() + 1 };
        let ph = if self.pane.is_some() { ((body - eh) * 55 / 100).max(4) } else { 0 };
        let dh = body.saturating_sub(eh + ph).max(1);
        self.diff_at = (x0, y0, dw, dh);
        self.pane_at = (x0, y0 + dh + eh, dw, ph);

        if sw > 0 {
            let rows: Vec<sidebar::Row> = self
                .files
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let col = match f.status {
                        'A' | '?' => OK,
                        'D' => BAD,
                        _ => WARN,
                    };
                    let mut r = sidebar::Row::new(f.name.clone());
                    r.mark = Some((f.status, col));
                    r.extra = format!("+{} -{}", f.add, f.del);
                    r.current = i == self.cur;
                    r.bold = self.fresh.get(i).is_some_and(|v| v.iter().any(|&b| b));
                    r.target = Some(i);
                    r
                })
                .collect();
            let sel = (!rows.is_empty()).then_some(self.cur);
            self.geo = sidebar::draw(&mut self.screen, &rows, sel, &mut self.side_st, (0, sw), (y0, body), !self.pane_focus);
        } else {
            self.geo = sidebar::Geo::default();
        }

        if self.help {
            for (i, l) in HELP.iter().enumerate().take(dh) {
                self.screen.puts(x0 + 2, y0 + 1 + i, l, Style::fg(TEXT), w);
            }
        } else if !self.loaded {
            self.screen.puts(x0 + 2, y0 + 1, "reading changes…", Style::new(DIM, 0, ITALIC), w);
        } else if self.files.is_empty() {
            self.screen.puts(x0 + 2, y0 + 1, &format!("no changes since {}; waiting for some", self.base), Style::new(DIM, 0, ITALIC), w);
        } else {
            self.draw_diff();
        }

        if eh > 0 {
            let ey = y0 + dh;
            for (i, l) in entry.iter().enumerate() {
                let y = ey + 1 + i;
                self.screen.fill(x0, w, y, Style::new(TEXT, BG_NOTE, 0));
                let st = if i == 0 && l.starts_with("## ") { Style::new(ACCENT, BG_NOTE, BOLD) } else { Style::new(TEXT, BG_NOTE, 0) };
                let l = if i == 0 { l.trim_start_matches("## ") } else { l };
                self.screen.puts(x0 + 2, y, l, st, w);
            }
            self.screen.fill(x0, w, ey, Style::new(DIM, BG_NOTE, 0));
            let age = self.notes_stamp.map_or(String::new(), |t| format!(" · {}", ago(t)));
            self.screen.puts(x0 + 1, ey, &format!("why{}", age), Style::new(DIM, BG_NOTE, ITALIC), w);
        }

        let mut cursor = None;
        if let Some(p) = &mut self.pane {
            let (px, py, pw, ph) = self.pane_at;
            let c = p.draw(&mut self.screen, (px, py), (pw, ph));
            if self.pane_focus {
                cursor = c;
            }
        }

        // status line
        self.screen.fill(0, w, h - 1, bar);
        let (text, fg) = if !self.err.is_empty() {
            (self.err.clone(), BAD)
        } else if !self.msg.is_empty() {
            (self.msg.clone(), WARN)
        } else if self.pane_focus {
            ("editing   Ctrl-W back to the diff   :w save   :q close the pane".to_string(), DIM)
        } else if self.pane.is_some() {
            ("Ctrl-W editor   Enter edit here   n N hunk   J K file   f follow   ? keys   q quit".to_string(), DIM)
        } else {
            ("Enter edit here   n N hunk   J K file   f follow   x why   ? keys   q quit".to_string(), DIM)
        };
        self.screen.puts(1, h - 1, &text, Style::new(fg, BAR, 0), w);
        self.screen.flush(cursor);
    }

    fn draw_diff(&mut self) {
        self.ensure_hl();
        let (x0, y0, w, h) = self.diff_at;
        let xr = x0 + w;
        let Some(f) = self.files.get(self.cur) else { return };
        let n = f.rows.len();
        if self.keep {
            let margin = 3.min(h / 4);
            if self.cursor < self.top + margin {
                self.top = self.cursor.saturating_sub(margin);
            }
            if self.cursor + margin >= self.top + h {
                self.top = (self.cursor + margin + 1).saturating_sub(h);
            }
        }
        self.top = self.top.min(n.saturating_sub(1));
        let gw = f.rows.iter().map(|r| r.old.max(r.new)).max().unwrap_or(0).max(1).to_string().len().max(3);
        let fresh = &self.fresh[self.cur];
        let tc = 2 * gw + 4;
        for (k, r) in f.rows.iter().enumerate().skip(self.top).take(h) {
            let y = y0 + k - self.top;
            let cur = k == self.cursor;
            match r.kind {
                RK::Hunk | RK::Note => {
                    let bg = if cur { BG_CUR } else { 0 };
                    self.screen.fill(x0, xr, y, Style::new(0, bg, 0));
                    let st = if r.kind == RK::Hunk { Style::new(HUNK, bg, 0) } else { Style::new(DIM, bg, ITALIC) };
                    self.screen.puts(x0 + 1, y, &r.text, st, xr);
                }
                RK::Add | RK::Del | RK::Ctx => {
                    let bg = match (r.kind, cur) {
                        (RK::Add, true) => BG_ADD_CUR,
                        (RK::Add, false) => BG_ADD,
                        (RK::Del, true) => BG_DEL_CUR,
                        (RK::Del, false) => BG_DEL,
                        (_, true) => BG_CUR,
                        _ => 0,
                    };
                    self.screen.fill(x0, xr, y, Style::new(0, bg, 0));
                    let num = |n: u32| if n > 0 { format!("{:>w$}", n, w = gw) } else { " ".repeat(gw) };
                    self.screen.puts(x0, y, &format!("{} {}", num(r.old), num(r.new)), Style::new(if cur { TEXT } else { DIM }, bg, 0), xr);
                    if fresh.get(k) == Some(&true) {
                        self.screen.put(x0 + 2 * gw + 1, y, '▎', Style::new(FRESH, bg, 0));
                    }
                    let (mark, col) = match r.kind {
                        RK::Add => ('+', OK),
                        RK::Del => ('-', BAD),
                        _ => (' ', 0),
                    };
                    self.screen.put(x0 + 2 * gw + 2, y, mark, Style::new(col, bg, BOLD));
                    let hl = self.hl.get(k);
                    let (mut x, mut c) = (x0 + tc, 0usize);
                    for (bi, ch) in r.text.char_indices() {
                        let (ch, cw) = if ch == '\t' { (' ', 4) } else { (ch, mtui::screen::char_width(ch)) };
                        if c >= self.hscroll {
                            if x + cw > xr {
                                break;
                            }
                            let st = syntax::style(hl.and_then(|v| v.get(bi)).copied().unwrap_or(0));
                            for _ in 0..if ch == ' ' { cw } else { 1 } {
                                x += self.screen.put(x, y, ch, Style::new(st.fg, bg, st.attr));
                            }
                        }
                        c += cw;
                    }
                }
            }
        }
        if n > h {
            let s = format!("{}% ", (self.top + h).min(n) * 100 / n);
            self.screen.puts(xr.saturating_sub(s.len() + 1), y0 + h - 1, &s, Style::fg(DIM), xr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::newest_entry;

    #[test]
    fn newest_first() {
        let log = "# Why\n<!-- note -->\n\n## b.rs:9 second\nwhy two\nmore\n\n## a.rs:3 first\nwhy one\n";
        assert_eq!(newest_entry(log), vec!["## b.rs:9 second", "why two", "more"]);
        assert!(newest_entry("# Why\n<!-- entries -->\n").is_empty());
    }
}
