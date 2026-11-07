// The viewer: the changed files down the side (by repo in a tree of
// checkouts), then a notes file and the code it points at, beside one mvi
// editor. A changed file opens in the editor with its diff drawn inline:
// what the parent branch had shows above where it was, added lines are
// tinted, unchanged stretches fold away. It is the file itself, so editing
// it works as in mvi and the diff follows as you type. The newest entry of
// an explanation log shows under the editor.

use crate::diff::{self, File, Head, RK};
use crate::notes::{self, Ref};
use mtui::screen::{Screen, Style, BOLD, ITALIC};
use mtui::sidebar;
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::wrap;
use mvi::editor::Mode;
use mvi::view::{Decor, Fold, Virt};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const TEXT: u8 = 252;
const DIM: u8 = 246;
const ACCENT: u8 = 180;
const OK: u8 = 114;
const BAD: u8 = 203;
const WARN: u8 = 209;
const FRESH: u8 = 220;
const BAR: u8 = 236;
const BG_ADD: u8 = 22;
const BG_DEL: u8 = 52;
const BG_RANGE: u8 = 24;
const BG_NOTE: u8 = 235;

/// Unchanged lines kept in view on each side of a change.
const CONTEXT: usize = 3;

const HELP: &[&str] = &[
    "The editor is mvi on the file itself; its diff is drawn in:",
    "  red rows above a line   what the parent branch had there (not part of the file)",
    "  green lines             added or changed     ▎ in the gutter: by the latest change",
    "  ▸ ⋯ rows                folded lines         blue lines: a note's reference",
    "",
    "In the editor (all of mvi, plus):",
    "  ]c [c       next / previous change      ]f [f    next / previous file in the list",
    "  za zo zc    toggle / open / close the fold at the cursor (a change folds too)",
    "  zR zM       open / close every fold     Enter or a click on a fold opens it",
    "  Enter       in the notes, on a line that refers to code: go there",
    "  Ctrl-W      to the list               :w save    :q quit",
    "",
    "In the list:",
    "  j k  g G    pick a file, the notes or a reference (it opens as you go)",
    "  Enter Tab   to the editor   f follow changes   x explanation   s hide the list   q quit",
];

/// What the editor shows, from the list.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Item {
    File(usize),
    Notes,
    Ref(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    List,
    Editor,
}

/// What a fold of the current layout stands for, by lines of the parent's
/// text, so its open state survives re-layouts as the file is edited.
#[derive(Clone, Copy)]
struct FoldKey {
    hunk: bool,
    old: (usize, usize),
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

    /// Whether the file at `p` (named `name` in the tree) is this one.
    fn is(&self, p: &Path, name: &str) -> bool {
        self.path == p || self.rel.as_deref() == Some(name)
    }
}

pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub suspend: bool,
    root: PathBuf,
    /// Several checkouts under `root`: the list groups files by repo.
    multi: bool,
    heads: Vec<Head>,
    files: Vec<File>,
    /// Per file, per row: added by the change that last touched the file.
    fresh: Vec<Vec<bool>>,
    loaded: bool,
    item: Option<Item>,
    focus: Focus,
    pane: Option<mvi::Pane>,
    /// Each changed file's text at its fork point, by name, with that fork.
    bases: HashMap<String, (String, Vec<String>)>,
    /// What the editor's decoration was built from: buffer, its version, `ver`.
    decor_key: Option<(PathBuf, u64, u64)>,
    decor_name: String,
    fold_keys: Vec<FoldKey>,
    /// Per file: unchanged stretches opened (by the parent's lines), and
    /// changes folded away (by where they start in the parent).
    opened: HashMap<String, Vec<(usize, usize)>>,
    shut: HashMap<String, HashSet<usize>>,
    /// Bumped whenever the snapshot or the notes change.
    ver: u64,
    follow: bool,
    side: bool,
    side_st: sidebar::State,
    geo: sidebar::Geo,
    /// The item index (into `items()`) of each sidebar row, from the last draw.
    side_targets: Vec<Option<usize>>,
    /// x, y, w, h of the editor, from the last draw.
    edit_at: (usize, usize, usize, usize),
    why: Option<Watched>,
    entry: Vec<String>,
    show_entry: bool,
    notes: Option<Watched>,
    notes_lines: Vec<String>,
    refs: Vec<Ref>,
    /// The notes changed since they were last looked at.
    notes_new: bool,
    /// `]` or `[` typed in the editor, waiting to see if `f` follows.
    prefix: Option<char>,
    last_key: Instant,
    help: bool,
    msg: String,
    err: String,
    /// A standing warning (no inotify), shown when nothing else is.
    note: String,
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

/// The inline diff of `lines` against `base` as editor decorations, with
/// the keys of its folds. Without a base only the reference range (`range`,
/// 1-based) and the notes lines that refer to code (`links`) are marked.
fn layout(lines: &[String], base: Option<&[String]>, fresh: &HashSet<&str>, opened: &[(usize, usize)], shut: &HashSet<usize>, range: Option<(usize, usize)>, links: &[usize]) -> (Option<Decor>, Vec<FoldKey>) {
    let n = lines.len();
    let mut d = Decor { bg: vec![0; n], mark: vec![(' ', 0); n], ..Decor::default() };
    let mut keys = Vec::new();
    let hunks = base.map(|b| diff::line_hunks(b, lines)).unwrap_or_default();
    // an unchanged stretch, all but CONTEXT lines next to a change folded away
    let stretch = |d: &mut Decor, keys: &mut Vec<FoldKey>, (s, e): (usize, usize), old: usize, before: bool, after: bool| {
        let a = s + if before { CONTEXT } else { 0 };
        let z = e.saturating_sub(if after { CONTEXT } else { 0 });
        if z >= a + 4 {
            let (oa, ob) = (old + (a - s), old + (z - s) - 1);
            let open = opened.iter().any(|&(x, y)| x <= ob && oa <= y);
            d.folds.push(Fold { a, b: z - 1, open, label: format!("{} unchanged lines", z - a), hides_above: false });
            keys.push(FoldKey { hunk: false, old: (oa, ob) });
        }
    };
    let (mut ne, mut oe) = (0, 0);
    for (i, h) in hunks.iter().enumerate() {
        stretch(&mut d, &mut keys, (ne, h.new.start), oe, i > 0, true);
        if let (false, Some(base)) = (h.old.is_empty(), base) {
            let rows = base[h.old.clone()].iter().map(|t| Virt { text: t.clone(), fg: 0, bg: BG_DEL, mark: ('-', BAD) }).collect();
            d.above.push((h.new.start, rows));
        }
        for l in h.new.clone() {
            d.bg[l] = BG_ADD;
            d.mark[l] = if fresh.contains(lines[l].as_str()) { ('▎', FRESH) } else { ('+', OK) };
        }
        d.changes.push(h.new.start.min(n.saturating_sub(1)));
        if !h.new.is_empty() {
            let label = format!("{} changed line{}, {} removed", h.new.len(), if h.new.len() == 1 { "" } else { "s" }, h.old.len());
            d.folds.push(Fold { a: h.new.start, b: h.new.end - 1, open: !shut.contains(&h.old.start), label, hides_above: true });
            keys.push(FoldKey { hunk: true, old: (h.old.start, h.old.end) });
        }
        (ne, oe) = (h.new.end, h.old.end);
    }
    if !hunks.is_empty() {
        stretch(&mut d, &mut keys, (ne, n), oe, true, false);
    }
    if let Some((a, b)) = range {
        for l in a.saturating_sub(1)..b.min(n) {
            if d.bg[l] == 0 {
                d.bg[l] = BG_RANGE;
            }
        }
    }
    for &l in links.iter().filter(|&&l| l < n) {
        d.mark[l] = ('›', ACCENT);
    }
    let plain = hunks.is_empty() && range.is_none() && links.is_empty();
    (if plain { None } else { Some(d) }, keys)
}

impl App {
    pub fn new(w: usize, h: usize, root: PathBuf, multi: bool, why: Option<PathBuf>, notes: Option<PathBuf>) -> App {
        let why = why.map(|p| Watched::new(p, &root));
        let notes = notes.map(|p| Watched::new(p, &root));
        App {
            screen: Screen::new(w, h),
            quit: false,
            suspend: false,
            root,
            multi,
            heads: vec![],
            files: vec![],
            fresh: vec![],
            loaded: false,
            item: None,
            focus: Focus::List,
            pane: None,
            bases: HashMap::new(),
            decor_key: None,
            decor_name: String::new(),
            fold_keys: vec![],
            opened: HashMap::new(),
            shut: HashMap::new(),
            ver: 0,
            follow: true,
            side: true,
            side_st: sidebar::State::default(),
            geo: sidebar::Geo::default(),
            side_targets: vec![],
            edit_at: (0, 0, 0, 0),
            why,
            entry: vec![],
            show_entry: true,
            notes,
            notes_lines: vec![],
            refs: vec![],
            notes_new: false,
            prefix: None,
            last_key: Instant::now() - Duration::from_secs(60),
            help: false,
            msg: String::new(),
            err: String::new(),
            note: String::new(),
        }
    }

    pub fn notice(&mut self, s: String) {
        self.note = s;
    }

    /// A new snapshot from the watcher.
    pub fn on_snapshot(&mut self, heads: Vec<Head>, mut files: Vec<File>) {
        self.err = heads
            .iter()
            .filter_map(|h| h.fork.as_ref().err().map(|e| if h.dir.is_empty() { format!("{}: {}", h.parent, e) } else { format!("{}: {}", h.dir, e) }))
            .collect::<Vec<_>>()
            .join("; ");
        self.heads = heads;
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
            Some(Item::File(i)) => self.files.get(i).map(|f| f.name.clone()),
            _ => None,
        };
        self.files = files;
        self.fresh = fresh;
        self.ver += 1;
        if let Some(n) = name {
            self.item = self.files.iter().position(|f| f.name == n).map(Item::File);
        }
        // a reference to a file that only now exists
        self.find_refs();
        self.reload_clean_buffers();
        match changed {
            Some((i, at)) if self.follow && self.can_follow() => {
                let line = self.files[i].rows.get(at).map(|r| if r.new > 0 { r.new } else { r.old.max(1) } as usize);
                self.show(Item::File(i), line);
            }
            Some((i, _)) => self.msg = format!("{} changed", self.files[i].name),
            None => {}
        }
        if self.pane.is_none() && !self.files.is_empty() {
            self.show(Item::File(0), None);
        }
        self.sync_item();
    }

    /// Follow a change only when it won't pull the rug from under the user:
    /// not mid-edit, not with unsaved changes, not just after a key.
    fn can_follow(&self) -> bool {
        let quiet = self.last_key.elapsed() > Duration::from_secs(3);
        match &self.pane {
            None => true,
            Some(p) => quiet && p.ed.mode == Mode::Normal && p.ed.pending.is_empty() && !p.ed.bb().dirty,
        }
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
        self.decor_key = None;
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
            self.notes_new = self.item != Some(Item::Notes) && self.ver > 0;
            self.find_refs();
            self.ver += 1;
            self.reload_clean_buffers();
        }
    }

    /// Re-read the references in the notes, keeping the selected one.
    fn find_refs(&mut self) {
        let key = |r: &Ref| (r.path.clone(), r.from, r.to);
        let was = match self.item {
            Some(Item::Ref(i)) => self.refs.get(i).map(key),
            _ => None,
        };
        let root = &self.root;
        self.refs = notes::refs(&self.notes_lines, &|p| root.join(p).is_file());
        if let Some(k) = was {
            self.item = Some(self.refs.iter().position(|r| key(r) == k).map_or(Item::Notes, Item::Ref));
        }
    }

    /// The list's items, in order.
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
        let i = self.item.and_then(|it| v.iter().position(|&x| x == it)).map_or(if d > 0 { -1 } else { v.len() as isize }, |i| i as isize);
        self.show(v[(i.saturating_add(d)).clamp(0, v.len() as isize - 1) as usize], None);
    }

    /// The name in the tree of the file at `p`.
    fn name_of(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().into_owned()
    }

    fn is_notes(&self, p: &Path) -> bool {
        self.notes.as_ref().is_some_and(|n| n.is(p, &self.name_of(p)))
    }

    /// Open `it` in the editor, at 1-based `line` (else where it starts).
    fn show(&mut self, it: Item, line: Option<usize>) {
        let (path, start) = match it {
            Item::File(i) => {
                let f = &self.files[i];
                if f.status != 'D' && !f.rows.is_empty() && f.rows.iter().all(|r| r.kind == RK::Note) {
                    self.msg = format!("{}: {}", f.name, f.rows[0].text);
                    return;
                }
                let first = f.rows.iter().find(|r| matches!(r.kind, RK::Add | RK::Del)).map_or(1, |r| if r.new > 0 { r.new } else { r.old.max(1) } as usize);
                (self.root.join(&f.name), first)
            }
            Item::Notes => {
                let Some(n) = &self.notes else { return };
                self.notes_new = false;
                (n.path.clone(), 1)
            }
            Item::Ref(r) => {
                let rf = &self.refs[r];
                (self.root.join(&rf.path), rf.from.max(1))
            }
        };
        let line = line.unwrap_or(start);
        match &mut self.pane {
            Some(p) => {
                p.ed.open_file(&path);
            }
            None => {
                let (w, h) = (self.edit_at.2.max(20), self.edit_at.3.max(5));
                self.pane = Some(mvi::Pane::open(w, h, &path, 1));
            }
        }
        self.item = Some(it);
        self.decor_key = None;
        // lay the diff out before placing the cursor, so it lands on a row that shows
        self.decorate();
        let th = self.edit_at.3.saturating_sub(2).max(1);
        if let Some(p) = &mut self.pane {
            let y = (line - 1).min(p.ed.bb().lines.len() - 1);
            p.ed.set_cursor((y, 0));
            let b = p.ed.b();
            if let Some(d) = &mut b.decor {
                if let Some(i) = d.closed(y) {
                    d.folds[i].open = true;
                }
            }
            // the change a third of the way down, its removed lines above it
            b.top = y.saturating_sub(th / 3);
        }
    }

    /// Keep the list on what the editor shows when the user switches files
    /// in the editor itself (`:e`, `:b`, `gf`).
    fn sync_item(&mut self) {
        let Some(path) = self.pane.as_ref().and_then(|p| p.ed.bb().path.clone()) else { return };
        let name = self.name_of(&path);
        let shows = match self.item {
            Some(Item::File(i)) => self.files.get(i).is_some_and(|f| f.name == name),
            Some(Item::Notes) => self.is_notes(&path),
            Some(Item::Ref(r)) => self.refs.get(r).is_some_and(|f| f.path == name),
            None => false,
        };
        if !shows {
            let notes = self.is_notes(&path);
            self.item = self.files.iter().position(|f| f.name == name).map(Item::File).or(notes.then_some(Item::Notes));
            self.decor_key = None;
        }
    }

    /// The parent's text of changed file `i`, cached per fork point.
    fn base_of(&mut self, i: usize) -> Vec<String> {
        let f = &self.files[i];
        let Some(head) = self.heads.get(f.repo) else { return vec![] };
        let Ok(fork) = &head.fork else { return vec![] };
        if let Some((k, v)) = self.bases.get(&f.name) {
            if k == fork {
                return v.clone();
            }
        }
        let lines = if matches!(f.status, 'A' | '?') {
            vec![]
        } else {
            let (root, path) = if head.dir.is_empty() { (self.root.clone(), f.name.clone()) } else { (self.root.join(&head.dir), f.name[head.dir.len() + 1..].to_string()) };
            diff::file_at(&root, fork, &path)
        };
        self.bases.insert(f.name.clone(), (fork.clone(), lines.clone()));
        lines
    }

    /// Lay the editor's current buffer out as an inline diff, when it or
    /// the snapshot changed since the last time.
    fn decorate(&mut self) {
        let Some(p) = &self.pane else { return };
        let b = p.ed.bb();
        let Some(path) = b.path.clone() else { return };
        let key = (path.clone(), b.version, self.ver);
        if self.decor_key.as_ref() == Some(&key) {
            return;
        }
        self.decor_key = Some(key);
        let name = self.name_of(&path);
        // what the user opened and closed in the last layout of this file
        if name == self.decor_name {
            if let Some(d) = &b.decor {
                if d.folds.len() == self.fold_keys.len() {
                    for (f, k) in d.folds.iter().zip(&self.fold_keys) {
                        if k.hunk {
                            let s = self.shut.entry(name.clone()).or_default();
                            if f.open {
                                s.remove(&k.old.0);
                            } else {
                                s.insert(k.old.0);
                            }
                        } else if f.open {
                            self.opened.entry(name.clone()).or_default().push(k.old);
                        }
                    }
                }
            }
        }
        let fi = self.files.iter().position(|f| f.name == name);
        let base = fi.map(|i| self.base_of(i));
        let Some(p) = &self.pane else { return };
        let lines = &p.ed.bb().lines;
        let fresh: HashSet<&str> = fi.map(|i| self.files[i].rows.iter().zip(&self.fresh[i]).filter(|&(r, &f)| f && r.kind == RK::Add).map(|(r, _)| r.text.as_str()).collect()).unwrap_or_default();
        let range = match self.item {
            Some(Item::Ref(r)) if self.refs[r].path == name && self.refs[r].from > 0 => Some((self.refs[r].from, self.refs[r].to.max(self.refs[r].from))),
            _ => None,
        };
        let links: Vec<usize> = if self.is_notes(&path) { self.refs.iter().map(|r| r.at).collect() } else { vec![] };
        let none = HashSet::new();
        let (decor, keys) = layout(lines, base.as_deref(), &fresh, self.opened.get(&name).map_or(&[][..], |v| v), self.shut.get(&name).unwrap_or(&none), range, &links);
        self.fold_keys = keys;
        self.decor_name = name;
        if let Some(p) = &mut self.pane {
            p.ed.b().decor = decor;
        }
    }

    /// The reference on the notes line under the editor's cursor.
    fn ref_at_cursor(&self) -> Option<usize> {
        let b = self.pane.as_ref()?.ed.bb();
        if !self.is_notes(b.path.as_deref()?) {
            return None;
        }
        self.refs.iter().position(|r| r.at == b.cy)
    }

    fn try_quit(&mut self) {
        let dirty: Vec<String> = self.pane.as_ref().map(|p| p.ed.bufs.iter().filter(|b| b.dirty).map(|b| b.name.clone()).collect()).unwrap_or_default();
        if dirty.is_empty() {
            self.quit = true;
        } else {
            self.msg = format!("unsaved: {} (:w in the editor, or :qa! there to drop them)", dirty.join(", "));
        }
    }

    // ---- keys ----

    pub fn handle_key(&mut self, k: Key) {
        self.last_key = Instant::now();
        if self.help {
            self.help = false;
            return;
        }
        if let Key::Mouse(m) = k {
            return self.mouse(m);
        }
        match self.focus {
            Focus::List => self.list_key(k),
            Focus::Editor => self.editor_key(k),
        }
    }

    fn list_key(&mut self, k: Key) {
        self.msg.clear();
        match k {
            Key::Char('q') | Key::Ctrl('c') => self.try_quit(),
            Key::Ctrl('z') => self.suspend = true,
            Key::Char('?') => self.help = true,
            Key::Char('j') | Key::Down => self.step(1),
            Key::Char('k') | Key::Up => self.step(-1),
            Key::Char('g') | Key::Home => self.step(isize::MIN),
            Key::Char('G') | Key::End => self.step(isize::MAX),
            Key::Enter | Key::Tab | Key::Char('l') | Key::Right | Key::Ctrl('w') if self.pane.is_some() => self.focus = Focus::Editor,
            Key::Char('f') => {
                self.follow = !self.follow;
                self.msg = if self.follow { "follow on: jumps to the next change" } else { "follow off" }.into();
            }
            Key::Char('x') => self.show_entry = !self.show_entry,
            Key::Char('s') => {
                self.side = !self.side;
                if !self.side && self.pane.is_some() {
                    self.focus = Focus::Editor;
                }
            }
            _ => {}
        }
    }

    fn editor_key(&mut self, k: Key) {
        let Some(p) = &self.pane else {
            self.focus = Focus::List;
            return;
        };
        let normal = p.ed.mode == Mode::Normal && p.ed.pending.is_empty();
        let mut keys = vec![k.clone()];
        if let Some(c) = self.prefix.take() {
            if k == Key::Char('f') {
                return self.step(if c == ']' { 1 } else { -1 });
            }
            keys.insert(0, Key::Char(c));
        } else if normal {
            match k {
                Key::Ctrl('w') => {
                    self.side = true;
                    self.focus = Focus::List;
                    return;
                }
                Key::Char(c @ (']' | '[')) => {
                    self.prefix = Some(c);
                    return;
                }
                Key::Enter => {
                    if let Some(r) = self.ref_at_cursor() {
                        return self.show(Item::Ref(r), None);
                    }
                }
                _ => {}
            }
        }
        if let Some(p) = &mut self.pane {
            for k in keys {
                p.key(k);
            }
            if p.finished() {
                self.quit = true;
            }
        }
        self.sync_item();
    }

    fn mouse(&mut self, m: Mouse) {
        let (x0, y0, w, h) = self.edit_at;
        if self.pane.is_some() && m.x >= x0 && m.x < x0 + w && m.y >= y0 && m.y < y0 + h {
            self.focus = Focus::Editor;
            if let Some(p) = &mut self.pane {
                p.key(Key::Mouse(Mouse { x: m.x - x0, y: m.y - y0, ..m }));
            }
            return self.sync_item();
        }
        if !self.geo.contains(m.x) {
            return;
        }
        match m.kind {
            MouseKind::WheelUp => self.side_st.scroll(-3),
            MouseKind::WheelDown => self.side_st.scroll(3),
            MouseKind::Press(0) => {
                let t = self.geo.row_at(m.x, m.y).and_then(|r| self.side_targets.get(r).copied().flatten());
                if let Some(it) = t.and_then(|t| self.items().get(t).copied()) {
                    self.focus = Focus::List;
                    self.show(it, None);
                }
            }
            _ => {}
        }
    }

    // ---- drawing ----

    /// What the changes are measured from: the parent branch, or each
    /// distinct one in a tree of checkouts; with one repo, also the fork point.
    fn since(&self) -> String {
        let short = |s: &str| if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) { s[..8].to_string() } else { s.to_string() };
        match self.heads.as_slice() {
            [] => "…".into(),
            [h] => match &h.fork {
                Ok(f) if !h.parent.starts_with(&f[..8.min(f.len())]) => format!("{} @ {}", short(&h.parent), &f[..8.min(f.len())]),
                _ => short(&h.parent),
            },
            hs => {
                let mut v: Vec<String> = hs.iter().map(|h| short(&h.parent)).collect();
                v.sort();
                v.dedup();
                v.join(", ")
            }
        }
    }

    fn side_rows(&mut self) -> (Vec<sidebar::Row>, Option<usize>) {
        let grouped = self.notes.is_some() || self.multi;
        let head = |t: &str| sidebar::Row { text: t.into(), dir: true, open: true, ..sidebar::Row::default() };
        let mut rows = vec![];
        let mut targets = vec![];
        let mut sel = None;
        if grouped && !self.multi {
            rows.push(head("changes"));
            targets.push(None);
        }
        if grouped && self.files.is_empty() {
            rows.push(sidebar::Row { text: "no changes yet".into(), depth: 1, dim: true, ..sidebar::Row::default() });
            targets.push(None);
        }
        let depth = usize::from(grouped);
        let mut repo = usize::MAX;
        for (t, it) in self.items().into_iter().enumerate() {
            let mut r = match it {
                Item::File(i) => {
                    let f = &self.files[i];
                    // in a tree of checkouts, a header for each repo with changes
                    if self.multi && f.repo != repo {
                        repo = f.repo;
                        let mut r = head(&self.heads.get(repo).map_or(String::new(), |h| h.dir.clone()));
                        r.extra = self.heads.get(repo).map_or(String::new(), |h| if h.branch.is_empty() { "detached".into() } else { h.branch.clone() });
                        rows.push(r);
                        targets.push(None);
                    }
                    let col = match f.status {
                        'A' | '?' => OK,
                        'D' => BAD,
                        _ => WARN,
                    };
                    let prefix = self.heads.get(f.repo).map_or(0, |h| if h.dir.is_empty() { 0 } else { h.dir.len() + 1 });
                    let mut r = sidebar::Row::new(if self.multi { &f.name[prefix.min(f.name.len())..] } else { f.name.as_str() });
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
            r.current = Some(it) == self.item;
            if Some(it) == self.item {
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
        if h < 6 || w < 30 {
            self.screen.flush(None);
            return;
        }
        let bar = Style::new(TEXT, BAR, 0);

        // title
        self.screen.fill(0, w, 0, bar);
        let name = self.root.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let x = self.screen.puts(1, 0, &format!("mdiff {}", name), Style::new(ACCENT, BAR, BOLD), w);
        self.screen.puts(x + 1, 0, &format!("since {}", self.since()), Style::new(DIM, BAR, 0), w);
        let (a, d) = self.files.iter().fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
        let nf = self.files.len();
        let repos = if self.multi { format!("{} repos  ", self.heads.len()) } else { String::new() };
        let right = format!("{}{} file{}  +{} -{}{}", repos, nf, if nf == 1 { "" } else { "s" }, a, d, if self.follow { "  follow" } else { "" });
        self.screen.puts(w.saturating_sub(right.chars().count() + 1), 0, &right, Style::new(TEXT, BAR, 0), w);

        // layout: list | editor, the explanation under the editor
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
            wrap(&text, dw.saturating_sub(3)).into_iter().map(|(a, b)| text[a..b].to_string()).take(6).collect()
        } else {
            vec![]
        };
        let eh = if entry.is_empty() { 0 } else { entry.len() + 1 };
        let ehh = body.saturating_sub(eh).max(4);
        self.edit_at = (x0, y0, dw, ehh);

        if sw > 0 {
            self.geo = sidebar::draw(&mut self.screen, &srows, sel, &mut self.side_st, (0, sw), (y0, body), self.focus == Focus::List);
        } else {
            self.geo = sidebar::Geo::default();
        }

        let mut cursor = None;
        if self.help {
            for (i, l) in HELP.iter().enumerate().take(ehh) {
                self.screen.puts(x0 + 2, y0 + 1 + i, l, Style::fg(TEXT), w);
            }
        } else if self.pane.is_some() {
            self.decorate();
            let focus = self.focus == Focus::Editor;
            if let Some(p) = &mut self.pane {
                let c = p.draw(&mut self.screen, (x0, y0), (dw, ehh));
                if focus {
                    cursor = c;
                }
            }
        } else {
            let t = if !self.loaded {
                "reading changes…".to_string()
            } else {
                format!("no changes since {}; waiting for some{}", self.since(), if self.notes.is_some() { "; j shows the notes" } else { "" })
            };
            self.screen.puts(x0 + 2, y0 + 1, &t, Style::new(DIM, 0, ITALIC), w);
        }

        if eh > 0 {
            let ey = y0 + ehh;
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

        // status line
        self.screen.fill(0, w, h - 1, bar);
        let (text, fg) = if !self.err.is_empty() {
            (self.err.clone(), BAD)
        } else if !self.msg.is_empty() {
            (self.msg.clone(), WARN)
        } else if !self.note.is_empty() {
            (format!("{}   ? keys", self.note), WARN)
        } else if self.focus == Focus::Editor {
            ("Ctrl-W list   ]c [c change   ]f [f file   za fold   zR zM all   :w save".to_string(), DIM)
        } else {
            ("j k pick   Enter edit   f follow   x why   s hide list   ? keys   q quit".to_string(), DIM)
        };
        self.screen.puts(1, h - 1, &text, Style::new(fg, BAR, 0), w);
        self.screen.flush(cursor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first() {
        let log = "# Why\n<!-- note -->\n\n## b.rs:9 second\nwhy two\nmore\n\n## a.rs:3 first\nwhy one\n";
        assert_eq!(newest_entry(log), vec!["## b.rs:9 second", "why two", "more"]);
        assert!(newest_entry("# Why\n<!-- entries -->\n").is_empty());
    }

    fn lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("l{}", i)).collect()
    }

    #[test]
    fn inline_diff_layout() {
        let base = lines(30);
        let mut new = base.clone();
        new[10] = "changed".into();
        new.remove(20);
        let none = HashSet::new();
        let (d, keys) = layout(&new, Some(&base), &HashSet::new(), &[], &none, None, &[]);
        let d = d.unwrap();
        // the old line 10 shows above the new one, line 20 above where it was
        assert_eq!(d.above.iter().map(|(l, v)| (*l, v[0].text.as_str())).collect::<Vec<_>>(), vec![(10, "l10"), (20, "l20")]);
        assert_eq!(d.bg[10], BG_ADD);
        assert_eq!(d.changes, vec![10, 20]);
        // unchanged: 0..=6 folded (3 kept above line 10), 14..=16 too short to fold, 23..=28 folded
        let folds: Vec<(usize, usize, bool)> = d.folds.iter().map(|f| (f.a, f.b, f.hides_above)).collect();
        assert_eq!(folds, vec![(0, 6, false), (10, 10, true), (23, 28, false)]);
        assert!(d.folds.iter().filter(|f| !f.hides_above).all(|f| !f.open));
        assert_eq!(keys.len(), 3);
        // an opened stretch stays open when it's laid out again
        let (d, _) = layout(&new, Some(&base), &HashSet::new(), &[keys[2].old], &none, None, &[]);
        assert!(d.unwrap().folds[2].open);
    }

    #[test]
    fn unchanged_file_has_no_decor() {
        let base = lines(5);
        let (d, _) = layout(&base, Some(&base), &HashSet::new(), &[], &HashSet::new(), None, &[]);
        assert!(d.is_none());
        let (d, _) = layout(&base, None, &HashSet::new(), &[], &HashSet::new(), Some((2, 3)), &[]);
        assert_eq!(d.unwrap().bg, vec![0, BG_RANGE, BG_RANGE, 0, 0]);
    }
}
