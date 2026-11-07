// The viewer: changed files down the side, then a notes file and the code
// it refers to. The main view shows the selected file's diff (syntax
// highlighted, the lines added by the latest change marked), the notes, or
// the referenced file with those lines picked out. Under it, the newest
// entry of an explanation log, and an mvi pane to edit at the cursor while
// everything above keeps updating.

use crate::diff::{self, File, Row, RK};
use crate::notes::{self, Ref};
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
const BG_RANGE: u8 = 24;
const BG_NOTE: u8 = 235;

const HELP: &[&str] = &[
    "j k  ↓ ↑       move          Ctrl-D Ctrl-U  half a page   g G  top, bottom",
    "J K  Tab       next / previous item in the list (files, notes, references)",
    "n N            next / previous hunk; in the notes, line with a reference;",
    "               in a referenced file, block of changed lines",
    "← →            scroll sideways",
    "Enter          edit the file at this line in an mvi pane below the view;",
    "               in the notes, go to the code the line refers to",
    "e              edit here (in the notes: edit the notes)",
    "Ctrl-W         switch between the view and the pane (from normal mode)",
    "               :w saves and the view updates; :q closes the pane",
    "f              follow: jump to whatever changes next (on at start)",
    "x              show / hide the latest explanation entry",
    "s              show / hide the list",
    "q              quit",
    "",
    "▎ marks lines added by the latest change, › notes lines that refer to code,",
    "and a blue background the lines a reference points at.",
];

/// What the main view shows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Item {
    File(usize),
    Notes,
    Ref(usize),
}

/// How a view's marked rows are drawn.
#[derive(Clone, Copy, PartialEq, Default)]
enum Mark {
    /// added by the latest change: a bar in the gutter
    #[default]
    Fresh,
    /// a notes line that refers to code
    Link,
    /// the lines a reference points at: a background
    Range,
}

/// The rows of the current item, rebuilt when it or the data changes.
#[derive(Default)]
struct View {
    rows: Vec<Row>,
    mark: Vec<bool>,
    hl: Vec<Vec<u8>>,
    kind: Mark,
}

/// A notes or log file: where it is, given and inside the tree, and when it
/// was last read.
struct Watched {
    path: PathBuf,
    rel: Option<String>,
    stamp: Option<SystemTime>,
}

impl Watched {
    fn new(path: PathBuf, root: &Path) -> Watched {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        // the file may not exist yet: resolve its directory instead
        let dir = canon(path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")));
        let rel = path.file_name().and_then(|n| dir.join(n).strip_prefix(canon(root)).ok().map(|r| r.to_string_lossy().into_owned()));
        Watched { path, rel, stamp: None }
    }

    /// The file's text when it changed since the last call.
    fn changed(&mut self) -> Option<String> {
        let m = mtime(&self.path);
        if m == self.stamp {
            return None;
        }
        self.stamp = m;
        Some(std::fs::read_to_string(&self.path).unwrap_or_default())
    }
}

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
    item: Item,
    view: View,
    view_key: Option<(Item, u64)>,
    /// Bumped whenever anything a view is built from changes.
    ver: u64,
    cursor: usize,
    top: usize,
    /// Bring the cursor into view on the next draw (false after wheel scrolls).
    keep: bool,
    hscroll: usize,
    follow: bool,
    side: bool,
    side_st: sidebar::State,
    geo: sidebar::Geo,
    /// The item index (into `items()`) of each sidebar row, from the last draw.
    side_targets: Vec<Option<usize>>,
    /// x, y, w, h of the view and of the pane, from the last draw.
    view_at: (usize, usize, usize, usize),
    pane_at: (usize, usize, usize, usize),
    why: Option<Watched>,
    entry: Vec<String>,
    show_entry: bool,
    notes: Option<Watched>,
    notes_lines: Vec<String>,
    refs: Vec<Ref>,
    /// The notes changed since they were last looked at.
    notes_new: bool,
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
    pub fn new(w: usize, h: usize, root: PathBuf, base: String, why: Option<PathBuf>, notes: Option<PathBuf>) -> App {
        let why = why.map(|p| Watched::new(p, &root));
        let notes = notes.map(|p| Watched::new(p, &root));
        App {
            screen: Screen::new(w, h),
            quit: false,
            suspend: false,
            root,
            base,
            files: vec![],
            fresh: vec![],
            loaded: false,
            item: Item::File(0),
            view: View::default(),
            view_key: None,
            ver: 0,
            cursor: 0,
            top: 0,
            keep: true,
            hscroll: 0,
            follow: true,
            side: true,
            side_st: sidebar::State::default(),
            geo: sidebar::Geo::default(),
            side_targets: vec![],
            view_at: (0, 0, 0, 0),
            pane_at: (0, 0, 0, 0),
            why,
            entry: vec![],
            show_entry: true,
            notes,
            notes_lines: vec![],
            refs: vec![],
            notes_new: false,
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
        for w in [&self.why, &self.notes].into_iter().flatten() {
            if let Some(n) = &w.rel {
                files.retain(|f| &f.name != n);
            }
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
        let name = match self.item {
            Item::File(i) => self.files.get(i).map(|f| f.name.clone()),
            _ => None,
        };
        self.files = files;
        self.fresh = fresh;
        self.ver += 1;
        match changed {
            // follow, unless the user is reading the notes or editing
            Some((i, at)) if self.follow && !self.pane_focus && matches!(self.item, Item::File(_)) => {
                self.item = Item::File(i);
                self.cursor = at;
                // put the change a third of the way down
                self.top = at.saturating_sub(self.view_at.3 / 3);
                self.keep = true;
            }
            Some((i, _)) if !matches!(self.item, Item::File(_)) => self.msg = format!("{} changed", self.files[i].name),
            _ => {
                if let Some(i) = name.and_then(|n| self.files.iter().position(|f| f.name == n)) {
                    self.item = Item::File(i);
                }
            }
        }
        if let Item::File(i) = self.item {
            self.item = Item::File(i.min(self.files.len().saturating_sub(1)));
        }
        // a reference to a file that only now exists
        self.find_refs();
        self.reload_clean_buffers();
    }

    /// Files the editor has open but not modified follow changes made on disk
    /// (by the other sessions), so `:w` never writes back a stale copy.
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

    /// Reload the explanation log and the notes when they change.
    pub fn tick(&mut self) {
        if let Some(s) = self.why.as_mut().and_then(|w| w.changed()) {
            self.entry = newest_entry(&s);
        }
        if let Some(s) = self.notes.as_mut().and_then(|w| w.changed()) {
            self.notes_lines = s.lines().map(String::from).collect();
            self.notes_new = self.item != Item::Notes && self.ver > 0;
            self.find_refs();
            self.ver += 1;
            self.reload_clean_buffers();
        }
    }

    /// Re-read the references in the notes, keeping the selected one.
    fn find_refs(&mut self) {
        let key = |r: &Ref| (r.path.clone(), r.from, r.to);
        let was = match self.item {
            Item::Ref(i) => self.refs.get(i).map(key),
            _ => None,
        };
        let root = &self.root;
        self.refs = notes::refs(&self.notes_lines, &|p| root.join(p).is_file());
        if let Some(k) = was {
            self.item = self.refs.iter().position(|r| key(r) == k).map_or(Item::Notes, Item::Ref);
        }
    }

    /// The sidebar's selectable items, in order.
    fn items(&self) -> Vec<Item> {
        let mut v: Vec<Item> = (0..self.files.len()).map(Item::File).collect();
        if self.notes.is_some() {
            v.push(Item::Notes);
            v.extend((0..self.refs.len()).map(Item::Ref));
        }
        v
    }

    fn step(&mut self, d: isize) {
        let v = self.items();
        if v.is_empty() {
            return;
        }
        let i = v.iter().position(|&x| x == self.item).map_or(if d > 0 { -1 } else { v.len() as isize }, |i| i as isize);
        self.select(v[(i + d).clamp(0, v.len() as isize - 1) as usize]);
    }

    fn select(&mut self, it: Item) {
        // picking a reference again goes back to its lines
        if it == self.item && !matches!(it, Item::Ref(_)) {
            return;
        }
        self.item = it;
        self.hscroll = 0;
        self.keep = true;
        (self.cursor, self.top) = (0, 0);
        match it {
            Item::Ref(r) => {
                let at = self.refs[r].from.saturating_sub(1);
                self.cursor = at;
                self.top = at.saturating_sub(self.view_at.3 / 3);
            }
            Item::Notes => self.notes_new = false,
            Item::File(_) => {}
        }
    }

    fn move_to(&mut self, i: usize) {
        self.cursor = i.min(self.view.rows.len().saturating_sub(1));
        self.keep = true;
    }

    /// n / N: the next hunk, notes line with a reference, or block of changed
    /// lines, whichever the view has.
    fn jump(&mut self, fwd: bool) {
        let v = &self.view;
        let stop = |k: usize| match v.kind {
            Mark::Fresh => v.rows[k].kind == RK::Hunk,
            Mark::Link => v.mark[k],
            Mark::Range => v.rows[k].kind == RK::Add && (k == 0 || v.rows[k - 1].kind != RK::Add),
        };
        let to = if fwd { (self.cursor + 1..v.rows.len()).find(|&k| stop(k)) } else { (0..self.cursor).rev().find(|&k| stop(k)) };
        match to {
            Some(k) => {
                self.cursor = k;
                self.top = k.saturating_sub(if v.kind == Mark::Fresh { 0 } else { self.view_at.3 / 3 });
            }
            None => self.msg = format!("nothing {}", if fwd { "below" } else { "above" }),
        }
    }

    // ---- editing ----

    /// Enter (`go`) or e: edit at the cursor; Enter on a notes line with a
    /// reference goes to it instead.
    fn edit(&mut self, go: bool) {
        self.ensure_view();
        match self.item {
            Item::File(i) => {
                let Some(f) = self.files.get(i) else { return };
                if f.status == 'D' {
                    self.msg = "the file is deleted: nothing to edit".into();
                    return;
                }
                let path = self.root.join(&f.name);
                self.open(&path, diff::edit_line(&f.rows, self.cursor));
            }
            Item::Notes => {
                if go {
                    if let Some(r) = self.refs.iter().position(|r| r.at == self.cursor) {
                        return self.select(Item::Ref(r));
                    }
                }
                let Some(path) = self.notes.as_ref().map(|n| n.path.clone()) else { return };
                self.open(&path, self.cursor + 1);
            }
            Item::Ref(r) => {
                let path = self.root.join(&self.refs[r].path);
                self.open(&path, diff::edit_line(&self.view.rows, self.cursor));
            }
        }
    }

    fn open(&mut self, path: &Path, line: usize) {
        match &mut self.pane {
            Some(p) => {
                p.ed.open_file(path);
                let y = (line - 1).min(p.ed.bb().lines.len() - 1);
                p.ed.set_cursor((y, 0));
            }
            None => self.pane = Some(mvi::Pane::open(80, 20, path, line)),
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
        self.ensure_view();
        let n = self.view.rows.len();
        let page = self.view_at.3.max(2);
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
            Key::Char('J') | Key::Tab => self.step(1),
            Key::Char('K') | Key::BackTab => self.step(-1),
            Key::Char('n') => self.jump(true),
            Key::Char('N') => self.jump(false),
            Key::Left => self.hscroll = self.hscroll.saturating_sub(8),
            Key::Right => self.hscroll += 8,
            Key::Enter => self.edit(true),
            Key::Char('e') => self.edit(false),
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
                self.top = (self.top + 3).min(self.view.rows.len().saturating_sub(1));
                self.keep = false;
            }
            MouseKind::Press(0) if side => {
                let t = self.geo.row_at(m.x, m.y).and_then(|r| self.side_targets.get(r).copied().flatten());
                if let Some(it) = t.and_then(|t| self.items().get(t).copied()) {
                    self.select(it);
                }
            }
            MouseKind::Press(0) => {
                let (_, y0, _, h) = self.view_at;
                if m.y >= y0 && m.y < y0 + h {
                    self.cursor = (self.top + m.y - y0).min(self.view.rows.len().saturating_sub(1));
                }
            }
            _ => {}
        }
    }

    // ---- views ----

    fn ensure_view(&mut self) {
        if self.view_key == Some((self.item, self.ver)) {
            return;
        }
        self.view_key = Some((self.item, self.ver));
        let line = |kind, n: usize, text: &str| Row { kind, old: 0, new: n as u32, text: text.to_string() };
        let note = |text: &str| line(RK::Note, 0, text);
        let (rows, mark, kind, name): (Vec<Row>, Vec<bool>, Mark, String) = match self.item {
            Item::File(i) => match self.files.get(i) {
                Some(f) => (f.rows.clone(), self.fresh[i].clone(), Mark::Fresh, f.name.clone()),
                None => (vec![], vec![], Mark::Fresh, String::new()),
            },
            Item::Notes => {
                let rows: Vec<Row> = if self.notes_lines.is_empty() {
                    vec![note("no notes yet")]
                } else {
                    self.notes_lines.iter().enumerate().map(|(i, l)| line(RK::Ctx, i + 1, l)).collect()
                };
                let at: HashSet<usize> = self.refs.iter().map(|r| r.at).collect();
                let mark = (0..rows.len()).map(|i| at.contains(&i)).collect();
                (rows, mark, Mark::Link, self.notes.as_ref().map_or(String::new(), |n| n.path.display().to_string()))
            }
            Item::Ref(r) => {
                let rf = &self.refs[r];
                // lines this session added to the file show as added
                let added: HashSet<u32> = self.files.iter().filter(|f| f.name == rf.path).flat_map(|f| f.rows.iter()).filter(|r| r.kind == RK::Add).map(|r| r.new).collect();
                let mut rows = vec![];
                match std::fs::read_to_string(self.root.join(&rf.path)) {
                    Ok(s) => rows.extend(s.lines().enumerate().map(|(i, l)| line(if added.contains(&(i as u32 + 1)) { RK::Add } else { RK::Ctx }, i + 1, l))),
                    Err(e) => rows.push(note(&format!("cannot read {}: {}", rf.path, e))),
                }
                let (a, b) = (rf.from, rf.to.max(rf.from));
                let mark = rows.iter().map(|r: &Row| r.kind != RK::Note && a > 0 && (a..=b).contains(&(r.new as usize))).collect();
                (rows, mark, Mark::Range, rf.path.clone())
            }
        };

        let first = rows.iter().find(|r| matches!(r.kind, RK::Add | RK::Ctx)).map_or("", |r| r.text.as_str());
        let lang = syntax::detect(Path::new(&name), first);
        // in a diff the new and old sides keep separate state through a hunk
        let (mut sn, mut so) = (State::Normal, State::Normal);
        let mut hls = Vec::with_capacity(rows.len());
        for r in &rows {
            let mut hl = Vec::new();
            match r.kind {
                RK::Hunk => (sn, so) = (State::Normal, State::Normal),
                RK::Add if kind == Mark::Fresh => sn = syntax::highlight(lang, &r.text, sn, &mut hl),
                RK::Del => so = syntax::highlight(lang, &r.text, so, &mut hl),
                RK::Add | RK::Ctx => {
                    sn = syntax::highlight(lang, &r.text, sn, &mut hl);
                    so = sn;
                }
                RK::Note => {}
            }
            hls.push(hl);
        }
        self.view = View { rows, mark, hl: hls, kind };
    }

    // ---- drawing ----

    fn side_rows(&mut self) -> (Vec<sidebar::Row>, Option<usize>) {
        let grouped = self.notes.is_some();
        let head = |t: &str| sidebar::Row { text: t.into(), dir: true, open: true, ..sidebar::Row::default() };
        let mut rows = vec![];
        let mut targets = vec![];
        let mut sel = None;
        if grouped {
            rows.push(head("changes"));
            targets.push(None);
            if self.files.is_empty() {
                rows.push(sidebar::Row { text: "none yet".into(), depth: 1, dim: true, ..sidebar::Row::default() });
                targets.push(None);
            }
        }
        let depth = usize::from(grouped);
        for (t, it) in self.items().into_iter().enumerate() {
            let mut r = match it {
                Item::File(i) => {
                    let f = &self.files[i];
                    let col = match f.status {
                        'A' | '?' => OK,
                        'D' => BAD,
                        _ => WARN,
                    };
                    let mut r = sidebar::Row::new(f.name.clone());
                    r.mark = Some((f.status, col));
                    r.extra = format!("+{} -{}", f.add, f.del);
                    r.bold = self.fresh.get(i).is_some_and(|v| v.iter().any(|&b| b));
                    r
                }
                Item::Notes => {
                    rows.push(head("notes"));
                    targets.push(None);
                    let name = self.notes.as_ref().and_then(|n| n.path.file_name()).map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    let mut r = sidebar::Row::new(name);
                    r.mark = Some(('≡', ACCENT));
                    r.extra = format!("{} refs", self.refs.len());
                    r.bold = self.notes_new;
                    r
                }
                Item::Ref(i) => {
                    let rf = &self.refs[i];
                    let mut r = sidebar::Row::new(rf.text());
                    r.mark = Some(('›', ACCENT));
                    if self.files.iter().any(|f| f.name == rf.path) {
                        (r.extra, r.extra_fg) = ("changed".into(), WARN);
                    }
                    r
                }
            };
            r.depth = depth;
            r.current = it == self.item;
            if it == self.item {
                sel = Some(rows.len());
            }
            rows.push(r);
            targets.push(Some(t));
        }
        self.side_targets = targets;
        (rows, sel)
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

        // layout: list | view, then the explanation, then the pane
        let (y0, body) = (1, h - 2);
        let (srows, sel) = self.side_rows();
        let sw = if self.side && w >= 70 {
            let longest = srows.iter().map(|r| r.text.chars().count() + r.extra.chars().count()).max().unwrap_or(10);
            (longest + 8).clamp(18, w / 3)
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
        // a referenced file gets the notes line it came from pinned above it
        let label = match self.item {
            Item::Ref(r) if !self.help && dh > 2 => Some(self.refs[r].label.clone()),
            _ => None,
        };
        self.view_at = if label.is_some() { (x0, y0 + 1, dw, dh - 1) } else { (x0, y0, dw, dh) };
        self.pane_at = (x0, y0 + dh + eh, dw, ph);

        if sw > 0 {
            self.geo = sidebar::draw(&mut self.screen, &srows, sel, &mut self.side_st, (0, sw), (y0, body), !self.pane_focus);
        } else {
            self.geo = sidebar::Geo::default();
        }

        let idle = |s: &mut Screen, t: &str| s.puts(x0 + 2, y0 + 1, t, Style::new(DIM, 0, ITALIC), w);
        if self.help {
            for (i, l) in HELP.iter().enumerate().take(dh) {
                self.screen.puts(x0 + 2, y0 + 1 + i, l, Style::fg(TEXT), w);
            }
        } else if !self.loaded {
            idle(&mut self.screen, "reading changes…");
        } else if matches!(self.item, Item::File(_)) && self.files.is_empty() {
            let more = if self.notes.is_some() { "; J shows the notes" } else { "" };
            idle(&mut self.screen, &format!("no changes since {}; waiting for some{}", self.base, more));
        } else {
            if let Some(l) = &label {
                self.screen.fill(x0, w, y0, Style::new(TEXT, BG_NOTE, 0));
                let x = self.screen.puts(x0 + 1, y0, "› ", Style::new(ACCENT, BG_NOTE, BOLD), w);
                self.screen.puts(x, y0, l, Style::new(TEXT, BG_NOTE, ITALIC), w);
            }
            self.draw_view();
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
            let age = self.why.as_ref().and_then(|w| w.stamp).map_or(String::new(), |t| format!(" · {}", ago(t)));
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
        let tail = if self.pane.is_some() { "Ctrl-W editor   ? keys   q quit" } else { "f follow   x why   ? keys   q quit" };
        let (text, fg) = if !self.err.is_empty() {
            (self.err.clone(), BAD)
        } else if !self.msg.is_empty() {
            (self.msg.clone(), WARN)
        } else if self.pane_focus {
            ("editing   Ctrl-W back to the view   :w save   :q close the pane".to_string(), DIM)
        } else {
            let keys = match self.item {
                Item::File(_) => "Enter edit here   n N hunk   J K item",
                Item::Notes => "Enter go to the code   e edit notes   n N reference   J K item",
                Item::Ref(_) => "Enter edit here   n N changed lines   J K item",
            };
            (format!("{}   {}", keys, tail), DIM)
        };
        self.screen.puts(1, h - 1, &text, Style::new(fg, BAR, 0), w);
        self.screen.flush(cursor);
    }

    fn draw_view(&mut self) {
        self.ensure_view();
        let (x0, y0, w, h) = self.view_at;
        let xr = x0 + w;
        let v = &self.view;
        let n = v.rows.len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
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
        let single = v.kind != Mark::Fresh;
        let gw = v.rows.iter().map(|r| r.old.max(r.new)).max().unwrap_or(0).max(1).to_string().len().max(3);
        let nums_w = if single { gw } else { 2 * gw + 1 };
        let tc = nums_w + 3;
        for (k, r) in v.rows.iter().enumerate().skip(self.top).take(h) {
            let y = y0 + k - self.top;
            let cur = k == self.cursor;
            let marked = v.mark.get(k) == Some(&true);
            match r.kind {
                RK::Hunk | RK::Note => {
                    let bg = if cur { BG_CUR } else { 0 };
                    self.screen.fill(x0, xr, y, Style::new(0, bg, 0));
                    let st = if r.kind == RK::Hunk { Style::new(HUNK, bg, 0) } else { Style::new(DIM, bg, ITALIC) };
                    self.screen.puts(x0 + 1, y, &r.text, st, xr);
                }
                RK::Add | RK::Del | RK::Ctx => {
                    let bg = match (r.kind, cur) {
                        (_, false) if marked && v.kind == Mark::Range => BG_RANGE,
                        (RK::Add, true) => BG_ADD_CUR,
                        (RK::Add, false) => BG_ADD,
                        (RK::Del, true) => BG_DEL_CUR,
                        (RK::Del, false) => BG_DEL,
                        (_, true) => BG_CUR,
                        _ => 0,
                    };
                    self.screen.fill(x0, xr, y, Style::new(0, bg, 0));
                    let num = |n: u32| if n > 0 { format!("{:>w$}", n, w = gw) } else { " ".repeat(gw) };
                    let nums = if single { num(r.new) } else { format!("{} {}", num(r.old), num(r.new)) };
                    self.screen.puts(x0, y, &nums, Style::new(if cur { TEXT } else { DIM }, bg, 0), xr);
                    match v.kind {
                        Mark::Fresh if marked => _ = self.screen.put(x0 + nums_w, y, '▎', Style::new(FRESH, bg, 0)),
                        Mark::Link if marked => _ = self.screen.put(x0 + nums_w, y, '›', Style::new(ACCENT, bg, BOLD)),
                        _ => {}
                    }
                    let (mark, col) = match r.kind {
                        RK::Add => ('+', OK),
                        RK::Del => ('-', BAD),
                        _ => (' ', 0),
                    };
                    self.screen.put(x0 + nums_w + 1, y, mark, Style::new(col, bg, BOLD));
                    let hl = v.hl.get(k);
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
