// Pull request review: the changed files as rows (diff or whole file) with
// syntax highlighting, inline comment threads, and a pending review.

use super::{name_color, App, FileItem, Mode, Purpose, ACCENT, BG_BAR, C_BAD, C_LINK, C_OK, FG_DIM};
use crate::api::{self, Req, Tag};
use mtui::json::Value;
use mtui::picker::Picker;
use mtui::screen::{Style, BOLD, ITALIC};
use mtui::syntax::{self, State};
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::wrap;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct FileDiff {
    pub name: String,
    pub status: String,
    pub add: u64,
    pub del: u64,
    pub patch: Option<String>,
}

impl FileDiff {
    pub fn from_json(v: &Value) -> FileDiff {
        FileDiff {
            name: v.get("filename").str().into(),
            status: v.get("status").str().into(),
            add: v.get("additions").num() as u64,
            del: v.get("deletions").num() as u64,
            patch: v.get("patch").opt_str().map(String::from),
        }
    }

    pub fn is_image(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        self.status != "removed" && (n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".jpeg"))
    }

    /// One letter for the file list.
    pub fn mark(&self) -> char {
        match self.status.as_str() {
            "added" => 'A',
            "removed" => 'D',
            "renamed" => 'R',
            _ => 'M',
        }
    }
}

#[derive(Clone)]
pub struct RComment {
    pub id: u64,
    pub reply_to: Option<u64>,
    pub path: String,
    /// None when the line no longer exists in the head (outdated).
    pub line: Option<u32>,
    pub right: bool,
    pub who: String,
    pub at: String,
    pub body: String,
}

impl RComment {
    pub fn from_json(v: &Value) -> RComment {
        RComment {
            id: v.get("id").num() as u64,
            reply_to: v.get("in_reply_to_id").num().ne(&0.0).then(|| v.get("in_reply_to_id").num() as u64),
            path: v.get("path").str().into(),
            line: Some(v.get("line").num() as u32).filter(|&l| l > 0),
            right: v.get("side").str() != "LEFT",
            who: v.path("user.login").str().into(),
            at: v.get("created_at").str().into(),
            body: v.get("body").str().into(),
        }
    }
}

/// A comment queued for the next submitted review.
#[derive(Clone)]
pub struct Pending {
    pub path: String,
    pub line: u32,
    pub start: Option<u32>,
    pub right: bool,
    pub body: String,
}

pub enum Content {
    Loading,
    Text(Vec<String>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RK {
    Hunk,
    Add,
    Del,
    Ctx,
    Note,
    CmtHead { pending: bool },
    CmtBody { pending: bool },
    Img { i: usize, of: usize },
}

impl RK {
    pub fn selectable(self) -> bool {
        matches!(self, RK::Hunk | RK::Add | RK::Del | RK::Ctx | RK::Note)
    }
    pub fn is_code(self) -> bool {
        matches!(self, RK::Add | RK::Del | RK::Ctx)
    }
}

pub struct VRow {
    pub kind: RK,
    pub old: u32,
    pub new: u32,
    pub text: String,
    /// Syntax class per byte of `text`.
    pub hl: Vec<u8>,
    /// Gallery key for image rows.
    pub key: String,
}

fn row(kind: RK, old: u32, new: u32, text: impl Into<String>) -> VRow {
    VRow { kind, old, new, text: text.into(), hl: Vec::new(), key: String::new() }
}

/// Per-PR review state, kept across reloads of the item.
#[derive(Default)]
pub struct Rev {
    pub files: Option<Vec<FileDiff>>,
    pub comments: Vec<RComment>,
    pub cur: usize,
    pub cursor: usize,
    pub top: usize,
    pub anchor: Option<usize>,
    pub full: bool,
    pub hscroll: usize,
    pub contents: HashMap<String, Content>,
    pub viewed: HashSet<String>,
    pub rows: Vec<VRow>,
    rows_key: Option<(usize, bool, usize, u64)>,
    /// Bumped whenever something the rows depend on changes.
    pub ver: u64,
    /// The cursor's line (old, new) before a rebuild, to find it again.
    pub pos: (u32, u32),
}

impl Rev {
    pub fn file(&self) -> Option<&FileDiff> {
        self.files.as_ref()?.get(self.cur)
    }

    pub fn select_file(&mut self, i: usize) {
        self.cur = i;
        self.cursor = 0;
        self.top = 0;
        self.anchor = None;
        self.hscroll = 0;
        self.pos = (0, 0);
        self.ver += 1;
    }

    /// Rebuild the rows if the file, mode, width or data changed, keeping the
    /// cursor on the same line.
    pub fn ensure(&mut self, w: usize, pending: &[Pending], img: &dyn Fn(&str) -> Option<usize>, sha: &str) {
        let key = (self.cur, self.full, w, self.ver);
        if self.rows_key == Some(key) {
            return;
        }
        let Some(f) = self.files.as_ref().and_then(|v| v.get(self.cur)) else { return };
        let full = if self.full { self.contents.get(&f.name) } else { None };
        let rows = build(f, full, &self.comments, pending, w, img, sha);
        let (old, new) = self.pos;
        self.cursor = if (old, new) == (0, 0) {
            rows.iter().position(|r| r.kind.selectable()).unwrap_or(0)
        } else {
            rows.iter().position(|r| r.kind.is_code() && (r.old, r.new) == (old, new)).or_else(|| rows.iter().position(|r| r.kind.is_code() && (r.new == new || r.old == old))).unwrap_or(self.cursor.min(rows.len().saturating_sub(1)))
        };
        self.rows = rows;
        self.rows_key = Some(key);
    }

    pub fn remember(&mut self) {
        self.pos = self.rows.get(self.cursor).map_or((0, 0), |r| if r.kind.is_code() { (r.old, r.new) } else { (0, 0) });
    }

    /// Move to the next/previous selectable row.
    pub fn step(&mut self, d: isize) {
        let mut i = self.cursor as isize;
        loop {
            let j = i + d.signum();
            if j < 0 || j as usize >= self.rows.len() {
                break;
            }
            i = j;
            if self.rows[i as usize].kind.selectable() {
                self.cursor = i as usize;
                if d.abs() > 1 {
                    return self.step(d - d.signum());
                }
                break;
            }
        }
        self.remember();
    }

    pub fn goto(&mut self, i: usize) {
        self.cursor = i.min(self.rows.len().saturating_sub(1));
        if !self.rows.get(self.cursor).is_some_and(|r| r.kind.selectable()) {
            self.step(1);
            if !self.rows.get(self.cursor).is_some_and(|r| r.kind.selectable()) {
                self.step(-1);
            }
        }
        self.remember();
    }

    /// The side and line numbers a comment on rows anchor..=cursor would
    /// attach to: (start, end, right).
    pub fn target(&self) -> Option<(Option<u32>, u32, bool)> {
        let cur = self.rows.get(self.cursor)?;
        if !cur.kind.is_code() {
            return None;
        }
        let right = cur.kind != RK::Del;
        let n = |r: &VRow| if right { r.new } else { r.old };
        let (a, b) = match self.anchor {
            Some(a) => (a.min(self.cursor), a.max(self.cursor)),
            None => (self.cursor, self.cursor),
        };
        let lines = self.rows[a..=b].iter().filter(|r| r.kind.is_code() && (right != (r.kind == RK::Del)) || (r.kind == RK::Ctx)).map(n).filter(|&l| l > 0);
        let lo = lines.clone().min()?;
        let hi = lines.max()?;
        Some(((lo != hi).then_some(lo), hi, right))
    }

    /// The top-level comment of the thread on the cursor's line.
    pub fn thread_at(&self) -> Option<u64> {
        let (r, f) = (self.rows.get(self.cursor)?, self.file()?);
        let (right, n) = (r.kind != RK::Del, if r.kind == RK::Del { r.old } else { r.new });
        self.comments.iter().find(|c| c.reply_to.is_none() && c.path == f.name && c.line == Some(n) && c.right == right).map(|c| c.id)
    }

    pub fn comment_count(&self, path: &str) -> usize {
        self.comments.iter().filter(|c| c.reply_to.is_none() && c.path == path).count()
    }
}

fn parse_hunk(h: &str) -> (u32, u32) {
    let num = |s: &str| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(1);
    let old = h.strip_prefix("@@ -").map_or(1, num);
    let new = h.split_once(" +").map_or(1, |x| num(x.1));
    (old, new)
}

/// Rows for a unified-diff patch.
pub fn parse_patch(p: &str) -> Vec<VRow> {
    let (mut old, mut new) = (0, 0);
    let mut out = Vec::new();
    for l in p.lines() {
        if l.starts_with("@@") {
            (old, new) = parse_hunk(l);
            out.push(row(RK::Hunk, 0, 0, l));
        } else if let Some(t) = l.strip_prefix('+') {
            out.push(row(RK::Add, 0, new, t));
            new += 1;
        } else if let Some(t) = l.strip_prefix('-') {
            out.push(row(RK::Del, old, 0, t));
            old += 1;
        } else if l.starts_with('\\') {
            continue;
        } else if !out.is_empty() {
            out.push(row(RK::Ctx, old, new, l.strip_prefix(' ').unwrap_or(l)));
            old += 1;
            new += 1;
        }
    }
    out
}

fn comment_rows(out: &mut Vec<VRow>, who: &str, at: &str, body: &str, pending: bool, w: usize, outdated: bool) {
    let age = if at.is_empty() { String::new() } else { format!(" · {} ago", super::age(at)) };
    let tag = if pending { " · pending" } else if outdated { " · outdated" } else { "" };
    out.push(row(RK::CmtHead { pending }, 0, 0, format!("{}{}{}", who, age, tag)));
    for l in body.lines() {
        for (a, b) in wrap(l.trim_end_matches('\r'), w.max(10)) {
            out.push(row(RK::CmtBody { pending }, 0, 0, &l[a..b]));
        }
    }
}

pub fn build(f: &FileDiff, full: Option<&Content>, comments: &[RComment], pending: &[Pending], w: usize, img: &dyn Fn(&str) -> Option<usize>, sha: &str) -> Vec<VRow> {
    let mut code: Vec<VRow> = Vec::new();
    if f.is_image() && f.patch.is_none() {
        code.push(row(RK::Note, 0, 0, format!("image {}", f.name)));
        let key = format!("blob:{}:{}", sha, f.name);
        let of = img(&key).unwrap_or(1);
        for i in 0..of {
            let mut r = row(RK::Img { i, of }, 0, 0, f.name.clone());
            r.key = key.clone();
            code.push(r);
        }
    } else if let Some(c) = full {
        match c {
            Content::Loading => code.push(row(RK::Note, 0, 0, "loading file…")),
            Content::Failed(e) => code.push(row(RK::Note, 0, 0, format!("cannot load file: {}", e))),
            Content::Text(lines) => {
                let added: HashSet<u32> = f.patch.as_deref().map(|p| parse_patch(p).iter().filter(|r| r.kind == RK::Add).map(|r| r.new).collect()).unwrap_or_default();
                for (i, l) in lines.iter().enumerate() {
                    let n = i as u32 + 1;
                    code.push(row(if added.contains(&n) { RK::Add } else { RK::Ctx }, n, n, l.clone()));
                }
            }
        }
    } else {
        match &f.patch {
            Some(p) => code = parse_patch(p),
            None => code.push(row(RK::Note, 0, 0, "no diff available (binary, renamed or too large); press o to open it on GitHub")),
        }
    }

    // syntax: new side and old side keep separate state through a hunk
    let first = code.iter().find(|r| r.kind.is_code()).map_or("", |r| r.text.as_str()).to_string();
    let lang = syntax::detect(Path::new(&f.name), &first);
    let (mut sn, mut so) = (State::Normal, State::Normal);
    for r in code.iter_mut() {
        if r.text.contains('\t') {
            r.text = r.text.replace('\t', "    ");
        }
        match r.kind {
            RK::Hunk => (sn, so) = (State::Normal, State::Normal),
            RK::Add => sn = syntax::highlight(lang, &r.text, sn, &mut r.hl),
            RK::Del => so = syntax::highlight(lang, &r.text, so, &mut r.hl),
            RK::Ctx => {
                sn = syntax::highlight(lang, &r.text, sn, &mut r.hl);
                so = sn;
            }
            _ => {}
        }
    }

    // comments after the line they're on
    let mine: Vec<&RComment> = comments.iter().filter(|c| c.path == f.name).collect();
    let pend: Vec<&Pending> = pending.iter().filter(|p| p.path == f.name).collect();
    let mut out = Vec::with_capacity(code.len());
    for c in mine.iter().filter(|c| c.reply_to.is_none() && c.line.is_none()) {
        comment_rows(&mut out, &c.who, &c.at, &c.body, false, w, true);
        for r in mine.iter().filter(|r| r.reply_to == Some(c.id)) {
            comment_rows(&mut out, &r.who, &r.at, &r.body, false, w, true);
        }
    }
    for r in code {
        let (right, n) = (r.kind != RK::Del, if r.kind == RK::Del { r.old } else { r.new });
        let is_code = r.kind.is_code();
        out.push(r);
        if !is_code {
            continue;
        }
        for c in mine.iter().filter(|c| c.reply_to.is_none() && c.line == Some(n) && c.right == right) {
            comment_rows(&mut out, &c.who, &c.at, &c.body, false, w, false);
            for r in mine.iter().filter(|r| r.reply_to == Some(c.id)) {
                comment_rows(&mut out, &r.who, &r.at, &r.body, false, w, false);
            }
        }
        for p in pend.iter().filter(|p| p.line == n && p.right == right) {
            comment_rows(&mut out, "you", "", &p.body, true, w, false);
        }
    }
    out
}

const BG_ADD: u8 = 22;
const BG_DEL: u8 = 52;
const BG_ADD_CUR: u8 = 28;
const BG_DEL_CUR: u8 = 88;
const BG_CUR: u8 = 238;
const BG_RANGE: u8 = 24;
const BG_CMT: u8 = 235;

fn digits(n: u32) -> usize {
    n.max(1).to_string().len()
}

impl App {
    pub(super) fn review_mouse(&mut self, m: Mouse) {
        let Some(d) = &mut self.detail else { return };
        match m.kind {
            MouseKind::WheelUp => d.rev.step(-3),
            MouseKind::WheelDown => d.rev.step(3),
            MouseKind::Press(0) if m.y >= 1 => {
                let i = d.rev.top + m.y - 1;
                if d.rev.rows.get(i).is_some_and(|r| r.kind.selectable()) {
                    d.rev.cursor = i;
                    d.rev.remember();
                }
            }
            _ => {}
        }
    }

    pub(super) fn review_key(&mut self, k: Key, g: bool) {
        let page = (self.screen.h.saturating_sub(3) / 2).max(1) as isize;
        let Some(d) = &mut self.detail else { return };
        let rev = &mut d.rev;
        let nfiles = rev.files.as_ref().map_or(0, |f| f.len());
        let go_file = |rev: &mut Rev, i: isize| {
            if nfiles > 0 {
                rev.select_file(i.rem_euclid(nfiles as isize) as usize);
            }
        };
        match k {
            Key::Esc if rev.anchor.is_some() => rev.anchor = None,
            Key::Char('q') | Key::Esc => d.review = false,
            Key::Char('?') => self.mode = Mode::Help,
            Key::Char('j') | Key::Down | Key::Ctrl('n') | Key::Enter => rev.step(1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => rev.step(-1),
            Key::Ctrl('d') | Key::PageDown | Key::Char(' ') | Key::Ctrl('f') => rev.step(page),
            Key::Ctrl('u') | Key::PageUp | Key::Ctrl('b') => rev.step(-page),
            Key::Char('G') | Key::End => rev.goto(usize::MAX),
            Key::Home => rev.goto(0),
            Key::Char('g') if g => rev.goto(0),
            Key::Char('g') => self.pending_g = true,
            Key::Char('h') | Key::Left => rev.hscroll = rev.hscroll.saturating_sub(8),
            Key::Char('l') | Key::Right => rev.hscroll += 8,
            Key::Char(']') | Key::Tab => go_file(rev, rev.cur as isize + 1),
            Key::Char('[') | Key::BackTab => go_file(rev, rev.cur as isize - 1),
            Key::Char('}') => {
                if let Some(i) = (rev.cursor + 1..rev.rows.len()).find(|&i| rev.rows[i].kind == RK::Hunk) {
                    rev.goto(i);
                }
            }
            Key::Char('{') => {
                if let Some(i) = (0..rev.cursor).rev().find(|&i| rev.rows[i].kind == RK::Hunk) {
                    rev.goto(i);
                }
            }
            Key::Char(c @ ('n' | 'N')) => {
                let heads: Vec<usize> = (0..rev.rows.len()).filter(|&i| matches!(rev.rows[i].kind, RK::CmtHead { pending: false }) && i > 0 && (i == 1 || !matches!(rev.rows[i - 1].kind, RK::CmtBody { .. } | RK::CmtHead { .. }))).collect();
                // the code row just above each thread
                let at = |h: usize| (0..h).rev().find(|&i| rev.rows[i].kind.is_code()).unwrap_or(0);
                let t = if c == 'n' { heads.iter().map(|&h| at(h)).find(|&i| i > rev.cursor) } else { heads.iter().map(|&h| at(h)).rev().find(|&i| i < rev.cursor) };
                match t {
                    Some(i) => rev.goto(i),
                    None => self.info("no more comments"),
                }
            }
            Key::Char('v') => rev.anchor = if rev.anchor.is_some() { None } else { Some(rev.cursor) },
            Key::Char('m') => {
                if let Some(name) = rev.file().map(|f| f.name.clone()) {
                    if !rev.viewed.remove(&name) {
                        rev.viewed.insert(name);
                        go_file(rev, rev.cur as isize + 1);
                    }
                }
            }
            Key::Char('f') => {
                let Some(files) = &rev.files else { return };
                let items = files
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let n = rev.comment_count(&f.name);
                        FileItem(format!("{} {}{}  +{} −{}{}", f.mark(), f.name, if n > 0 { format!("  💬{}", n) } else { String::new() }, f.add, f.del, if rev.viewed.contains(&f.name) { "  ✓" } else { "" }), i)
                    })
                    .collect();
                self.mode = Mode::Files(Picker::new("files", items));
            }
            Key::Char('e') => {
                if rev.full {
                    rev.full = false;
                    rev.remember();
                    rev.ver += 1;
                    return;
                }
                let sha = d.pull.path("head.sha").str().to_string();
                let Some(name) = rev.file().map(|f| f.name.clone()) else { return };
                if sha.is_empty() {
                    return self.error("still loading the pull request");
                }
                rev.full = true;
                rev.remember();
                rev.ver += 1;
                if !rev.contents.contains_key(&name) {
                    rev.contents.insert(name.clone(), Content::Loading);
                    let path = name.split('/').map(mhttp::urlencode).collect::<Vec<_>>().join("/");
                    let req = Req { accept: api::RAW, ..Req::get(format!("/repos/{}/contents/{}?ref={}", d.item.repo, path, mhttp::urlencode(&sha))) };
                    let g = d.gen;
                    self.call(Tag::Content(g, name), req);
                }
            }
            Key::Char('c') => match (rev.target(), rev.file().map(|f| f.name.clone())) {
                (Some((start, line, right)), Some(path)) => self.compose(Purpose::Inline { path, line, start, right }),
                _ => self.error("put the cursor on a code line (v selects several)"),
            },
            Key::Char('r') => match rev.thread_at() {
                Some(id) => self.compose(Purpose::Reply(id)),
                None => self.error("no comment thread on this line"),
            },
            Key::Char('x') => {
                let (path, n, right) = match (rev.file().map(|f| f.name.clone()), rev.rows.get(rev.cursor)) {
                    (Some(p), Some(r)) if r.kind.is_code() => (p, if r.kind == RK::Del { r.old } else { r.new }, r.kind != RK::Del),
                    _ => return,
                };
                let key = (d.item.repo.clone(), d.item.num);
                let list = self.drafts.entry(key).or_default();
                match list.iter().rposition(|p| p.path == path && p.line == n && p.right == right) {
                    Some(i) => {
                        list.remove(i);
                        rev.ver += 1;
                        self.info("pending comment dropped");
                    }
                    None => self.error("no pending comment on this line"),
                }
            }
            Key::Char('S') => self.compose(Purpose::Review),
            Key::Char('a') => self.compose(Purpose::Approve),
            Key::Char('X') => self.compose(Purpose::Changes),
            Key::Char('o') => {
                let u = format!("{}/files", d.item.url);
                self.open_url(&u);
            }
            Key::Char('y') => self.yank(),
            _ => {}
        }
    }

    pub(super) fn render_review(&mut self, bottom: usize) {
        let w = self.screen.w;
        let view_h = bottom.saturating_sub(1);
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let Some(d) = &mut self.detail else { return };
        let drafts: &[Pending] = self.drafts.get(&(d.item.repo.clone(), d.item.num)).map(|v| v.as_slice()).unwrap_or(&[]);
        let sha = d.pull.path("head.sha").str().to_string();
        let title = format!(" {} #{} ", d.item.repo, d.item.num);
        self.screen.puts(0, 0, &title, Style::new(255, BG_BAR, BOLD), w);
        let rev = &mut d.rev;
        self.screen.set_images(Vec::new());
        let nfiles = rev.files.as_ref().map_or(0, |f| f.len());
        if nfiles == 0 {
            let s = if rev.files.is_none() { "loading files…" } else { "no changed files" };
            self.screen.puts(2, 2, s, Style::new(FG_DIM, 0, ITALIC), w);
            return;
        }
        let gallery = &self.gallery;
        let img_cols = w.saturating_sub(16).clamp(10, 60);
        rev.ensure(w.saturating_sub(14), drafts, &|k| gallery.size(k, img_cols, 14).map(|s| s.1), &sha);

        // header: file, position, stats
        if let Some(f) = rev.file() {
            let viewed = rev.viewed.contains(&f.name);
            let t = format!("{} ({}/{}) +{} −{}{}{}", f.name, rev.cur + 1, nfiles, f.add, f.del, if rev.full { "  whole file" } else { "" }, if viewed { "  ✓ viewed" } else { "" });
            self.screen.puts(str_w(&title), 0, &t, Style::new(252, BG_BAR, 0), w);
        }

        let n = rev.rows.len();
        let margin = 3.min(view_h / 4);
        if rev.cursor < rev.top + margin {
            rev.top = rev.cursor.saturating_sub(margin);
        }
        if rev.cursor + margin >= rev.top + view_h {
            rev.top = (rev.cursor + margin + 1).saturating_sub(view_h);
        }
        rev.top = rev.top.min(n.saturating_sub(view_h));
        let gw = digits(rev.rows.iter().map(|r| r.old.max(r.new)).max().unwrap_or(0)).max(3);
        let cx = 2 * gw + 3; // where the +/- marker sits
        let (lo, hi) = match rev.anchor {
            Some(a) => (a.min(rev.cursor), a.max(rev.cursor)),
            None => (usize::MAX, 0),
        };
        let mut pics: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
        for (k, r) in rev.rows.iter().enumerate().skip(rev.top).take(view_h) {
            let y = 1 + k - rev.top;
            let cur = k == rev.cursor;
            let ranged = (lo..=hi).contains(&k);
            match r.kind {
                RK::Hunk => {
                    let bg = if cur { BG_CUR } else if ranged { BG_RANGE } else { 0 };
                    self.screen.fill(0, w, y, Style::new(0, bg, 0));
                    self.screen.puts(0, y, &r.text, Style::new(73, bg, 0), w);
                }
                RK::Note => {
                    let bg = if cur { BG_CUR } else { 0 };
                    self.screen.fill(0, w, y, Style::new(0, bg, 0));
                    self.screen.puts(1, y, &r.text, Style::new(FG_DIM, bg, ITALIC), w);
                }
                RK::Add | RK::Del | RK::Ctx => {
                    let bg = match (r.kind, cur, ranged) {
                        (_, _, true) if !cur => BG_RANGE,
                        (RK::Add, true, _) => BG_ADD_CUR,
                        (RK::Add, ..) => BG_ADD,
                        (RK::Del, true, _) => BG_DEL_CUR,
                        (RK::Del, ..) => BG_DEL,
                        (_, true, _) => BG_CUR,
                        _ => 0,
                    };
                    self.screen.fill(0, w, y, Style::new(0, bg, 0));
                    let num = |n: u32| if n > 0 { format!("{:>w$}", n, w = gw) } else { " ".repeat(gw) };
                    self.screen.puts(0, y, &format!("{} {}", num(r.old), num(r.new)), Style::new(if cur { 252 } else { FG_DIM }, bg, 0), w);
                    let (mark, mc) = match r.kind {
                        RK::Add => ('+', C_OK),
                        RK::Del => ('-', C_BAD),
                        _ => (' ', 0),
                    };
                    self.screen.put(cx - 1, y, mark, Style::new(mc, bg, BOLD));
                    let (mut x, mut col) = (cx + 1, 0usize);
                    for (bi, ch) in r.text.char_indices() {
                        let cw = mtui::screen::char_width(ch);
                        if col >= rev.hscroll {
                            if x + cw > w {
                                break;
                            }
                            let st = syntax::style(*r.hl.get(bi).unwrap_or(&0));
                            x += self.screen.put(x, y, ch, Style::new(st.fg, bg, st.attr));
                        }
                        col += cw;
                    }
                }
                RK::CmtHead { pending } | RK::CmtBody { pending } => {
                    let bar = if pending { ACCENT } else { 238 };
                    self.screen.fill(cx, w, y, Style::new(0, BG_CMT, 0));
                    self.screen.put(cx, y, '┃', Style::new(bar, BG_CMT, 0));
                    if matches!(r.kind, RK::CmtHead { .. }) {
                        let who = r.text.split(' ').next().unwrap_or("");
                        let st = Style::new(if pending { ACCENT } else { name_color(who) }, BG_CMT, BOLD);
                        self.screen.puts(cx + 2, y, &r.text, st, w);
                    } else {
                        self.screen.puts(cx + 2, y, &r.text, Style::new(250, BG_CMT, 0), w);
                    }
                }
                RK::Img { i, of } => {
                    if self.gallery.size(&r.key, img_cols, 14).is_some() {
                        let e = pics.entry(r.key.clone()).or_insert((y, i, i, of));
                        e.2 = i + 1;
                    } else {
                        if self.gallery.request(&r.key) {
                            let (repo, path) = (d.item.repo.clone(), r.text.clone());
                            let enc = path.split('/').map(mhttp::urlencode).collect::<Vec<_>>().join("/");
                            let req = Req { accept: api::RAW, ..Req::get(format!("/repos/{}/contents/{}?ref={}", repo, enc, mhttp::urlencode(&sha))) };
                            self.pending += 1;
                            self.net.call(Tag::Blob(r.key.clone()), req);
                        }
                        let note = match self.gallery.slot(&r.key) {
                            Some(mimg::Slot::Failed(e)) => format!(" ({})", e),
                            Some(mimg::Slot::Loading) => " …".to_string(),
                            _ => " (set MTUI_IMAGES=1 to force image support)".to_string(),
                        };
                        self.screen.puts(cx, y, &format!("🖼 {}{}", r.text, note), Style::new(C_LINK, 0, 0), w);
                    }
                }
            }
        }
        let mut ims = Vec::new();
        if !matches!(self.mode, Mode::Pick(_) | Mode::Files(_) | Mode::Help) {
            for (key, (y, i0, i1, of)) in pics {
                if let Some((cols, _)) = self.gallery.size(&key, img_cols, 14) {
                    ims.extend(self.gallery.place(&key, 1, (cx, y), cols, of, i0, i1));
                }
            }
        }
        self.screen.set_images(ims);
        if n > view_h {
            let s = format!("{}% ", (rev.top + view_h).min(n) * 100 / n);
            self.screen.puts(w.saturating_sub(s.len()), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
    }
}

fn str_w(s: &str) -> usize {
    mtui::wrap::str_width(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -1,3 +1,4 @@ fn main\n a\n-b\n+B\n+c\n d\n\\ No newline at end of file\n@@ -10 +11 @@\n-x\n+y";

    fn rows() -> Vec<VRow> {
        parse_patch(PATCH)
    }

    #[test]
    fn numbering() {
        let r = rows();
        let k: Vec<(RK, u32, u32)> = r.iter().map(|r| (r.kind, r.old, r.new)).collect();
        assert_eq!(k, vec![(RK::Hunk, 0, 0), (RK::Ctx, 1, 1), (RK::Del, 2, 0), (RK::Add, 0, 2), (RK::Add, 0, 3), (RK::Ctx, 3, 4), (RK::Hunk, 0, 0), (RK::Del, 10, 0), (RK::Add, 0, 11)]);
        assert_eq!(r[1].text, "a");
    }

    fn file() -> FileDiff {
        FileDiff { name: "src/a.rs".into(), status: "modified".into(), add: 3, del: 2, patch: Some(PATCH.into()) }
    }

    fn cm(id: u64, reply: Option<u64>, line: Option<u32>, right: bool, body: &str) -> RComment {
        RComment { id, reply_to: reply, path: "src/a.rs".into(), line, right, who: "bob".into(), at: String::new(), body: body.into() }
    }

    #[test]
    fn comments_attach() {
        let cs = vec![cm(1, None, Some(3), true, "why?"), cm(2, Some(1), Some(3), true, "because"), cm(3, None, Some(10), false, "old line"), cm(4, None, None, true, "gone")];
        let p = vec![Pending { path: "src/a.rs".into(), line: 11, start: None, right: true, body: "mine".into() }];
        let out = build(&file(), None, &cs, &p, 40, &|_| None, "sha");
        let kinds: Vec<String> = out.iter().map(|r| format!("{:?}:{}", r.kind, r.text)).collect();
        // outdated first, then the thread under new line 3, the LEFT comment under old 10, pending under new 11
        assert!(kinds[0].starts_with("CmtHead") && kinds[1].contains("gone"));
        let at = |s: &str| kinds.iter().position(|k| k.contains(s)).unwrap();
        assert!(at("Add:c") < at("why?") && at("why?") < at("because") && at("because") < at("Ctx:d"));
        assert!(at("Del:x") < at("old line") && at("old line") < at("Add:y") && at("Add:y") < at("mine"));
    }

    #[test]
    fn targets() {
        let mut rev = Rev::default();
        rev.rows = rows();
        rev.cursor = 4; // Add new 3
        assert_eq!(rev.target(), Some((None, 3, true)));
        rev.anchor = Some(2); // from Del old 2 .. Add new 3 => right side lines 2..3
        assert_eq!(rev.target(), Some((Some(2), 3, true)));
        rev.anchor = None;
        rev.cursor = 7; // Del old 10
        assert_eq!(rev.target(), Some((None, 10, false)));
        rev.cursor = 0;
        assert_eq!(rev.target(), None);
    }

    #[test]
    fn stepping() {
        let mut rev = Rev::default();
        rev.rows = rows();
        rev.cursor = 1;
        rev.step(1);
        assert_eq!(rev.cursor, 2);
        rev.step(10);
        assert_eq!(rev.cursor, 8);
        rev.step(-100);
        assert_eq!(rev.cursor, 0);
    }
}
