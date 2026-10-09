// Editor state, key dispatch, and modal editing (normal / insert / visual).

use crate::buffer::{Buffer, Pos};
use crate::complete::{self, Completion};
use crate::diag::{CheckResult, Diag, Sev};
use crate::picker::{PickItem, Picker};
use mtui::picker::Pick;
use mtui::regex::{is_word, Regex};
use mtui::screen::{char_width, Screen};
use mtui::term::{Key, Mouse, MouseKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Cmd(char), // ':' '/' '?'
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Del,
    Change,
    Yank,
    Indent,
    Dedent,
    Lower,
    Upper,
    Toggle,
    Comment,
    /// gq: reflow to 'textwidth', cursor to the last line
    Format,
    /// gw: reflow, cursor stays
    FormatKeep,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TK {
    Excl,
    Incl,
    Line,
}

#[derive(Clone, Copy)]
pub struct Target {
    pub a: Pos,
    pub b: Pos,
    pub kind: TK,
}

#[derive(Clone, Copy, Debug)]
enum M {
    Left,
    Right,
    Up,
    Down,
    WordF(bool),
    WordB(bool),
    WordE(bool),
    WordGE(bool),
    LineStart,
    Fnb,
    LineEnd,
    FileStart,
    FileEnd,
    Find(char, char),
    RepFind(bool),
    Pair,
    ParaF,
    ParaB,
    ScrTop,
    ScrMid,
    ScrBot,
    SearchN(bool),
    Star(bool),
    Mark(char, bool),
    Obj(bool, char),
    DownFnb,
    UpFnb,
    Column,
}

enum P<T> {
    Ok(T),
    Pending,
    Invalid,
}

#[derive(Clone, Default)]
pub struct Reg {
    pub text: String,
    pub line: bool,
}

pub struct Opts {
    pub number: bool,
    pub relnum: bool,
    pub tabstop: usize,
    pub textwidth: usize,
    pub autocheck: bool,
    pub autocomplete: bool,
    pub hlsearch: bool,
    pub list: bool,
    pub mouse: bool,
    pub checks: HashMap<String, Vec<String>>,
}

pub struct Editor {
    pub bufs: Vec<Buffer>,
    pub cur: usize,
    pub alt: usize,
    pub mode: Mode,
    pub screen: Screen,
    pub pending: Vec<Key>,
    pub cmdline: String,
    pub hist: Vec<String>,
    pub hist_idx: usize,
    pub msg: String,
    pub msg_err: bool,
    pub msg_lines: Vec<String>,
    pub regs: HashMap<char, Reg>,
    pub search: Option<Regex>,
    pub search_pat: String,
    pub search_fwd: bool,
    pub show_hl: bool,
    pub search_origin: (Pos, usize),
    pub vstart: Pos,
    pub last_vis: Option<(Pos, Pos, Mode)>,
    last_find: Option<(char, char)>,
    pub last_jump: Pos,
    dot: Vec<Key>,
    dot_rec: Option<Vec<Key>>,
    replaying: bool,
    mac_rec: Option<(char, Vec<Key>)>,
    macros: HashMap<char, Vec<Key>>,
    last_macro: char,
    pub comp: Option<Completion>,
    sym_cache: Option<HashMap<String, String>>,
    pub picker: Option<Picker>,
    pub preview: Option<crate::preview::Preview>,
    /// The live browser preview (`:mmd`), when running.
    pub live: Option<crate::live::Live>,
    pub check_id: u64,
    pub check_running: bool,
    pub check_cwd: PathBuf,
    pub check_tx: Sender<CheckResult>,
    check_rx: Receiver<CheckResult>,
    pub qf: Vec<(PathBuf, Diag)>,
    pub qf_idx: usize,
    pub opts: Opts,
    pub quit: bool,
    pub hl_scratch: Vec<u8>,
    ins_reg_pending: bool,
    pub suspend: bool,
    /// Last left click (time, position, 1-3 for single/double/triple) and
    /// where a drag would start from.
    last_click: Option<(std::time::Instant, Pos, u8)>,
    drag_from: Option<Pos>,
}

fn kc(k: &Key) -> Option<char> {
    match k {
        Key::Char(c) => Some(*c),
        _ => None,
    }
}

fn cls(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || is_word(c) {
        1
    } else {
        2
    }
}

pub fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

pub fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

pub fn disp_col(s: &str, byte: usize, ts: usize) -> usize {
    let mut d = 0;
    for (i, c) in s.char_indices() {
        if i >= byte {
            break;
        }
        d += if c == '\t' { ts - d % ts } else { char_width(c) };
    }
    d
}

pub fn byte_at_disp(s: &str, want: usize, ts: usize) -> usize {
    let mut d = 0;
    for (i, c) in s.char_indices() {
        let w = if c == '\t' { ts - d % ts } else { char_width(c) };
        if d + w > want {
            return i;
        }
        d += w;
    }
    s.len()
}

fn indent_of(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

fn fnb(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

fn parse_count(keys: &[Key], mut i: usize) -> (Option<usize>, usize) {
    let mut n: Option<usize> = None;
    while let Some(Key::Char(c)) = keys.get(i) {
        if c.is_ascii_digit() && (n.is_some() || *c != '0') {
            n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(*c as usize - '0' as usize).min(100000));
            i += 1;
        } else {
            break;
        }
    }
    (n, i)
}

fn parse_motion(keys: &[Key], i: usize, textobj: bool) -> P<(M, usize)> {
    let next = |j: usize| -> Option<char> { keys.get(j).and_then(kc) };
    let m = match &keys[i] {
        Key::Left | Key::Backspace => M::Left,
        Key::Right => M::Right,
        Key::Up => M::Up,
        Key::Down => M::Down,
        Key::Home => M::LineStart,
        Key::End => M::LineEnd,
        Key::Enter => M::DownFnb,
        Key::Char(c) => match c {
            'h' => M::Left,
            'l' => M::Right,
            'j' => M::Down,
            'k' => M::Up,
            'w' => M::WordF(false),
            'W' => M::WordF(true),
            'b' => M::WordB(false),
            'B' => M::WordB(true),
            'e' => M::WordE(false),
            'E' => M::WordE(true),
            '0' => M::LineStart,
            '^' | '_' => M::Fnb,
            '$' => M::LineEnd,
            'G' => M::FileEnd,
            '%' => M::Pair,
            '}' => M::ParaF,
            '{' => M::ParaB,
            'H' => M::ScrTop,
            'M' => M::ScrMid,
            'L' => M::ScrBot,
            'n' => M::SearchN(false),
            'N' => M::SearchN(true),
            '*' => M::Star(true),
            '#' => M::Star(false),
            ';' => M::RepFind(false),
            ',' => M::RepFind(true),
            '+' => M::DownFnb,
            '-' => M::UpFnb,
            '|' => M::Column,
            'f' | 'F' | 't' | 'T' => match keys.get(i + 1) {
                None => return P::Pending,
                Some(Key::Char(t)) => return P::Ok((M::Find(*c, *t), i + 2)),
                Some(Key::Tab) => return P::Ok((M::Find(*c, '\t'), i + 2)),
                _ => return P::Invalid,
            },
            'g' => match next(i + 1) {
                None if keys.len() <= i + 1 => return P::Pending,
                Some('g') => return P::Ok((M::FileStart, i + 2)),
                Some('e') => return P::Ok((M::WordGE(false), i + 2)),
                Some('E') => return P::Ok((M::WordGE(true), i + 2)),
                _ => return P::Invalid,
            },
            '\'' | '`' => match next(i + 1) {
                None if keys.len() <= i + 1 => return P::Pending,
                Some(m) => return P::Ok((M::Mark(m, *c == '`'), i + 2)),
                None => return P::Invalid,
            },
            'i' | 'a' if textobj => match next(i + 1) {
                None if keys.len() <= i + 1 => return P::Pending,
                Some(o) => return P::Ok((M::Obj(*c == 'i', o), i + 2)),
                None => return P::Invalid,
            },
            _ => return P::Invalid,
        },
        _ => return P::Invalid,
    };
    P::Ok((m, i + 1))
}

pub fn regex_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

impl Editor {
    pub fn new(w: usize, h: usize) -> Editor {
        let (tx, rx) = channel();
        Editor {
            bufs: vec![Buffer::new(None)],
            cur: 0,
            alt: 0,
            mode: Mode::Normal,
            screen: Screen::new(w, h),
            pending: Vec::new(),
            cmdline: String::new(),
            hist: Vec::new(),
            hist_idx: 0,
            msg: String::new(),
            msg_err: false,
            msg_lines: Vec::new(),
            regs: HashMap::new(),
            search: None,
            search_pat: String::new(),
            search_fwd: true,
            show_hl: false,
            search_origin: ((0, 0), 0),
            vstart: (0, 0),
            last_vis: None,
            last_find: None,
            last_jump: (0, 0),
            dot: Vec::new(),
            dot_rec: None,
            replaying: false,
            mac_rec: None,
            macros: HashMap::new(),
            last_macro: 'q',
            comp: None,
            sym_cache: None,
            picker: None,
            preview: None,
            live: None,
            check_id: 0,
            check_running: false,
            check_cwd: PathBuf::new(),
            check_tx: tx,
            check_rx: rx,
            qf: Vec::new(),
            qf_idx: 0,
            opts: Opts { number: true, relnum: false, tabstop: 4, textwidth: 79, autocheck: true, autocomplete: true, hlsearch: true, list: false, mouse: true, checks: HashMap::new() },
            quit: false,
            hl_scratch: Vec::new(),
            ins_reg_pending: false,
            suspend: false,
            last_click: None,
            drag_from: None,
        }
    }

    pub fn recording_reg(&self) -> Option<char> {
        self.mac_rec.as_ref().map(|m| m.0)
    }
    pub fn b(&mut self) -> &mut Buffer {
        &mut self.bufs[self.cur]
    }
    pub fn bb(&self) -> &Buffer {
        &self.bufs[self.cur]
    }
    pub fn cursor(&self) -> Pos {
        (self.bb().cy, self.bb().cx)
    }
    pub fn info(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.msg_err = false;
    }
    pub fn err(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.msg_err = true;
    }
    pub fn text_h(&self) -> usize {
        self.screen.h.saturating_sub(2).max(1)
    }

    pub fn set_cursor(&mut self, p: Pos) {
        let ts = self.opts.tabstop;
        let b = self.b();
        b.cy = p.0.min(b.lines.len() - 1);
        b.cx = p.1.min(b.lines[b.cy].len());
        while !b.lines[b.cy].is_char_boundary(b.cx) {
            b.cx -= 1;
        }
        b.want_x = disp_col(&b.lines[b.cy], b.cx, ts);
    }

    pub fn clamp(&mut self) {
        let normal = !matches!(self.mode, Mode::Insert | Mode::Visual | Mode::VisualLine);
        let b = self.b();
        b.cy = b.cy.min(b.lines.len() - 1);
        let l = &b.lines[b.cy];
        b.cx = b.cx.min(l.len());
        while !l.is_char_boundary(b.cx) {
            b.cx -= 1;
        }
        if normal && b.cx == l.len() && b.cx > 0 {
            b.cx = prev_boundary(l, b.cx);
        }
    }

    // ---------- position helpers ----------
    fn ch(&self, p: Pos) -> char {
        self.bb().lines[p.0][p.1..].chars().next().unwrap_or('\n')
    }
    fn nextp(&self, p: Pos) -> Option<Pos> {
        let b = self.bb();
        let l = &b.lines[p.0];
        if p.1 < l.len() {
            Some((p.0, next_boundary(l, p.1)))
        } else if p.0 + 1 < b.lines.len() {
            Some((p.0 + 1, 0))
        } else {
            None
        }
    }
    fn prevp(&self, p: Pos) -> Option<Pos> {
        let b = self.bb();
        if p.1 > 0 {
            Some((p.0, prev_boundary(&b.lines[p.0], p.1)))
        } else if p.0 > 0 {
            Some((p.0 - 1, b.lines[p.0 - 1].len()))
        } else {
            None
        }
    }

    fn word_fwd(&self, mut p: Pos, big: bool) -> Pos {
        let start = p;
        let c0 = cls(self.ch(p), big);
        if c0 != 0 {
            while cls(self.ch(p), big) == c0 && p.1 < self.bb().lines[p.0].len() {
                match self.nextp(p) {
                    Some(n) => p = n,
                    None => return p,
                }
            }
        }
        loop {
            if cls(self.ch(p), big) != 0 {
                return p;
            }
            if self.bb().lines[p.0].is_empty() && p != start {
                return p;
            }
            match self.nextp(p) {
                Some(n) => p = n,
                None => return p,
            }
        }
    }

    fn word_end(&self, p: Pos, big: bool) -> Pos {
        let Some(mut p) = self.nextp(p) else { return p };
        while cls(self.ch(p), big) == 0 {
            match self.nextp(p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        let c = cls(self.ch(p), big);
        while let Some(n) = self.nextp(p) {
            if cls(self.ch(n), big) != c || n.0 != p.0 {
                break;
            }
            p = n;
        }
        p
    }

    fn word_back(&self, p: Pos, big: bool) -> Pos {
        let Some(mut p) = self.prevp(p) else { return p };
        while cls(self.ch(p), big) == 0 {
            if self.bb().lines[p.0].is_empty() {
                return p;
            }
            match self.prevp(p) {
                Some(q) => p = q,
                None => return p,
            }
        }
        let c = cls(self.ch(p), big);
        while let Some(q) = self.prevp(p) {
            if cls(self.ch(q), big) != c || q.0 != p.0 {
                break;
            }
            p = q;
        }
        p
    }

    fn word_ge(&self, p: Pos, big: bool) -> Pos {
        // back to end of previous word
        let c0 = cls(self.ch(p), big);
        let mut p = p;
        while let Some(q) = self.prevp(p) {
            p = q;
            if cls(self.ch(p), big) != c0 {
                break;
            }
        }
        while cls(self.ch(p), big) == 0 {
            match self.prevp(p) {
                Some(q) => p = q,
                None => return p,
            }
        }
        p
    }

    pub fn match_pair(&self, p: Pos) -> Option<Pos> {
        let c = self.ch(p);
        let (open, close, fwd) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => return None,
        };
        let b = self.bb();
        let mut depth = 0i32;
        if fwd {
            let end = (p.0 + 20000).min(b.lines.len());
            for ln in p.0..end {
                let l = &b.lines[ln];
                let s = if ln == p.0 { p.1 } else { 0 };
                for (i, ch) in l[s..].char_indices() {
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((ln, s + i));
                        }
                    }
                }
            }
        } else {
            let lo = p.0.saturating_sub(20000);
            for ln in (lo..=p.0).rev() {
                let l = &b.lines[ln];
                let e = if ln == p.0 { next_boundary(l, p.1) } else { l.len() };
                for (i, ch) in l[..e].char_indices().rev() {
                    if ch == close {
                        depth += 1;
                    } else if ch == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((ln, i));
                        }
                    }
                }
            }
        }
        None
    }

    fn enclosing(&self, p: Pos, open: char, close: char) -> Option<(Pos, Pos)> {
        let c = self.ch(p);
        let op = if c == open {
            p
        } else if c == close {
            self.match_pair(p)?
        } else {
            let b = self.bb();
            let mut depth = 0;
            let mut found = None;
            'outer: for ln in (p.0.saturating_sub(20000)..=p.0).rev() {
                let l = &b.lines[ln];
                let e = if ln == p.0 { p.1 } else { l.len() };
                for (i, ch) in l[..e].char_indices().rev() {
                    if ch == close {
                        depth += 1;
                    } else if ch == open {
                        if depth == 0 {
                            found = Some((ln, i));
                            break 'outer;
                        }
                        depth -= 1;
                    }
                }
            }
            found?
        };
        let cl = self.match_pair(op)?;
        Some((op, cl))
    }

    fn text_object(&self, inner: bool, o: char, n: usize) -> Option<Target> {
        let (cy, cx) = self.cursor();
        let b = self.bb();
        let line = &b.lines[cy];
        match o {
            'w' | 'W' => {
                let big = o == 'W';
                if line.is_empty() {
                    return Some(Target { a: (cy, 0), b: (cy, 0), kind: TK::Excl });
                }
                let c = cls(self.ch((cy, cx)), big);
                let mut s = cx;
                while s > 0 {
                    let q = prev_boundary(line, s);
                    if cls(line[q..].chars().next().unwrap(), big) != c {
                        break;
                    }
                    s = q;
                }
                let mut e = cx;
                for _ in 0..n {
                    let c = cls(line[e..].chars().next().unwrap_or('\n'), big);
                    while e < line.len() && cls(line[e..].chars().next().unwrap(), big) == c {
                        e = next_boundary(line, e);
                    }
                    if !inner && c != 0 {
                        let e0 = e;
                        while e < line.len() && line[e..].starts_with(char::is_whitespace) {
                            e = next_boundary(line, e);
                        }
                        if e == e0 {
                            while s > 0 && line[..s].ends_with(char::is_whitespace) {
                                s = prev_boundary(line, s);
                            }
                        }
                    }
                }
                Some(Target { a: (cy, s), b: (cy, e), kind: TK::Excl })
            }
            '"' | '\'' | '`' => {
                let qs: Vec<usize> = line.char_indices().filter(|&(i, c)| c == o && (i == 0 || !line[..i].ends_with('\\'))).map(|(i, _)| i).collect();
                let mut pair = None;
                for ch in qs.chunks(2) {
                    if ch.len() == 2 && ch[1] >= cx {
                        pair = Some((ch[0], ch[1]));
                        break;
                    }
                }
                let (q1, q2) = pair?;
                if inner {
                    Some(Target { a: (cy, q1 + 1), b: (cy, q2), kind: TK::Excl })
                } else {
                    let mut e = q2 + 1;
                    while e < line.len() && line.as_bytes()[e] == b' ' {
                        e += 1;
                    }
                    Some(Target { a: (cy, q1), b: (cy, e), kind: TK::Excl })
                }
            }
            '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' => {
                let (op, cl) = match o {
                    '(' | ')' | 'b' => ('(', ')'),
                    '[' | ']' => ('[', ']'),
                    '<' | '>' => ('<', '>'),
                    _ => ('{', '}'),
                };
                let mut p = (cy, cx);
                let mut r = self.enclosing(p, op, cl)?;
                for _ in 1..n {
                    p = self.prevp(r.0)?;
                    r = self.enclosing(p, op, cl)?;
                }
                let (a, z) = r;
                if !inner {
                    return Some(Target { a, b: (z.0, z.1 + 1), kind: TK::Excl });
                }
                let a2 = (a.0, a.1 + 1);
                let b = self.bb();
                if a2.1 == b.lines[a.0].len() && b.lines[z.0][..z.1].trim().is_empty() && z.0 > a.0 + 1 {
                    return Some(Target { a: (a.0 + 1, 0), b: (z.0 - 1, 0), kind: TK::Line });
                }
                Some(Target { a: a2, b: z, kind: TK::Excl })
            }
            'p' => {
                let blank = |i: usize| b.lines[i].trim().is_empty();
                let kind = blank(cy);
                let mut s = cy;
                while s > 0 && blank(s - 1) == kind {
                    s -= 1;
                }
                let mut e = cy;
                for k in 0..n {
                    if k > 0 && e + 1 < b.lines.len() {
                        e += 1;
                    }
                    let kk = blank(e);
                    while e + 1 < b.lines.len() && blank(e + 1) == kk {
                        e += 1;
                    }
                }
                if !inner && e + 1 < b.lines.len() {
                    e += 1;
                    while e + 1 < b.lines.len() && blank(e + 1) {
                        e += 1;
                    }
                }
                Some(Target { a: (s, 0), b: (e, 0), kind: TK::Line })
            }
            _ => None,
        }
    }

    fn find_char(&self, kind: char, t: char, n: usize) -> Option<Pos> {
        let (cy, cx) = self.cursor();
        let l = &self.bb().lines[cy];
        let mut x = cx;
        for i in 0..n {
            match kind {
                'f' | 't' => {
                    let mut start = next_boundary(l, x);
                    if kind == 't' && i == 0 && start < l.len() && l[start..].starts_with(t) && n == 1 {
                        // already just before the target: skip it (like ; repeat)
                        start = next_boundary(l, start);
                    }
                    let off = l[start..].find(t)?;
                    x = start + off;
                }
                _ => {
                    let mut end = x;
                    if kind == 'T' && i == 0 && end > 0 && l[..end].ends_with(t) {
                        end = prev_boundary(l, end);
                    }
                    x = l[..end].rfind(t)?;
                }
            }
        }
        Some(match kind {
            't' => (cy, prev_boundary(l, x)),
            'T' => (cy, next_boundary(l, x)),
            _ => (cy, x),
        })
    }

    pub fn search_from(&self, from: Pos, fwd: bool, re: &Regex) -> Option<(Pos, Pos)> {
        let b = self.bb();
        let n = b.lines.len();
        if fwd {
            let l = &b.lines[from.0];
            let st = next_boundary(l, from.1);
            if st <= l.len() {
                if let Some((s, e)) = re.find_at(l, st).filter(|m| m.0 > from.1 || from.1 >= l.len()) {
                    return Some(((from.0, s), (from.0, e)));
                }
            }
            for k in 1..=n {
                let ln = (from.0 + k) % n;
                if let Some((s, e)) = re.find_at(&b.lines[ln], 0) {
                    return Some(((ln, s), (ln, e)));
                }
            }
        } else {
            if let Some((s, e)) = re.rfind_before(&b.lines[from.0], from.1) {
                return Some(((from.0, s), (from.0, e)));
            }
            for k in 1..=n {
                let ln = (from.0 + n - k) % n;
                let l = &b.lines[ln];
                if let Some((s, e)) = re.rfind_before(l, l.len() + 1) {
                    return Some(((ln, s), (ln, e)));
                }
            }
        }
        None
    }

    pub fn word_under_cursor(&self) -> Option<String> {
        let (cy, cx) = self.cursor();
        let l = &self.bb().lines[cy];
        let mut s = cx;
        // move forward to a word char if not on one
        while s < l.len() && !is_word(l[s..].chars().next().unwrap()) {
            s = next_boundary(l, s);
        }
        if s >= l.len() {
            return None;
        }
        let mut a = s;
        while a > 0 && is_word(l[prev_boundary(l, a)..].chars().next().unwrap()) {
            a = prev_boundary(l, a);
        }
        let mut e = s;
        while e < l.len() && is_word(l[e..].chars().next().unwrap()) {
            e = next_boundary(l, e);
        }
        Some(l[a..e].to_string())
    }

    pub fn set_search(&mut self, pat: &str, fwd: bool) -> bool {
        match Regex::new(pat, true) {
            Ok(r) => {
                self.search = Some(r);
                self.search_pat = pat.to_string();
                self.search_fwd = fwd;
                self.show_hl = true;
                true
            }
            Err(e) => {
                self.err(format!("bad pattern: {}", e));
                false
            }
        }
    }

    fn eval_motion(&mut self, m: M, n: usize, has_count: bool, op: bool) -> Option<Target> {
        let cur = self.cursor();
        let (cy, cx) = cur;
        let b = self.bb();
        let nl = b.lines.len();
        let l = &b.lines[cy];
        let t = |p: Pos, kind: TK| Some(Target { a: cur, b: p, kind });
        match m {
            M::Left => {
                let mut x = cx;
                for _ in 0..n {
                    x = prev_boundary(l, x);
                }
                t((cy, x), TK::Excl)
            }
            M::Right => {
                let mut x = cx;
                for _ in 0..n {
                    x = next_boundary(l, x);
                }
                if !op && x >= l.len() {
                    x = prev_boundary(l, l.len());
                }
                t((cy, x), TK::Excl)
            }
            M::Up => {
                if cy == 0 && !op {
                    return None;
                }
                t((cy.saturating_sub(n), cx), TK::Line)
            }
            M::Down => {
                if cy + 1 >= nl && !op {
                    return None;
                }
                t(((cy + n).min(nl - 1), cx), TK::Line)
            }
            M::DownFnb => {
                let y = (cy + n).min(nl - 1);
                t((y, fnb(&b.lines[y])), TK::Line)
            }
            M::UpFnb => {
                let y = cy.saturating_sub(n);
                t((y, fnb(&b.lines[y])), TK::Line)
            }
            M::WordF(big) => {
                let mut p = cur;
                for i in 0..n {
                    let q = self.word_fwd(p, big);
                    if op && i == n - 1 && q.0 > p.0 {
                        // dw on the last word of a line stops at the line end
                        p = (p.0, self.bb().lines[p.0].len());
                        if p.1 == 0 || self.bb().lines[p.0][..p.1].trim().is_empty() && p.0 != cy {
                            p = q;
                        }
                        break;
                    }
                    p = q;
                }
                t(p, TK::Excl)
            }
            M::WordB(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = self.word_back(p, big);
                }
                t(p, TK::Excl)
            }
            M::WordE(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = self.word_end(p, big);
                }
                t(p, TK::Incl)
            }
            M::WordGE(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = self.word_ge(p, big);
                }
                t(p, TK::Incl)
            }
            M::LineStart => t((cy, 0), TK::Excl),
            M::Fnb => t((cy, fnb(l)), TK::Excl),
            M::LineEnd => {
                let y = (cy + n - 1).min(nl - 1);
                let ll = &b.lines[y];
                let x = if op { ll.len() } else { prev_boundary(ll, ll.len()) };
                let r = Target { a: cur, b: (y, x), kind: if op { TK::Excl } else { TK::Incl } };
                if !op {
                    self.b().want_x = usize::MAX;
                }
                Some(r)
            }
            M::FileStart | M::FileEnd => {
                let y = if has_count { n.saturating_sub(1).min(nl - 1) } else if matches!(m, M::FileStart) { 0 } else { nl - 1 };
                let x = fnb(&b.lines[y]);
                self.last_jump = cur;
                t((y, x), TK::Line)
            }
            M::Column => t((cy, byte_at_disp(l, n.saturating_sub(1), self.opts.tabstop)), TK::Excl),
            M::Find(k, c) => {
                self.last_find = Some((k, c));
                let p = self.find_char(k, c, n)?;
                t(p, if k == 'f' || k == 't' { TK::Incl } else { TK::Excl })
            }
            M::RepFind(rev) => {
                let (k, c) = self.last_find?;
                let k2 = if rev {
                    match k {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    }
                } else {
                    k
                };
                let p = self.find_char(k2, c, n)?;
                t(p, if k2 == 'f' || k2 == 't' { TK::Incl } else { TK::Excl })
            }
            M::Pair => {
                if has_count {
                    let y = (nl * n.min(100) + 99) / 100;
                    let y = y.saturating_sub(1).min(nl - 1);
                    return t((y, fnb(&b.lines[y])), TK::Line);
                }
                // find first bracket at or after cursor on this line
                let off = l[cx..].find(|c: char| "()[]{}".contains(c))?;
                let p = self.match_pair((cy, cx + off))?;
                self.last_jump = cur;
                t(p, TK::Incl)
            }
            M::ParaF | M::ParaB => {
                let blank = |i: usize| b.lines[i].trim().is_empty();
                let mut y = cy;
                for _ in 0..n {
                    if matches!(m, M::ParaF) {
                        while y + 1 < nl && blank(y + 1) {
                            y += 1;
                        }
                        while y + 1 < nl && !blank(y + 1) {
                            y += 1;
                        }
                        y = (y + 1).min(nl - 1);
                    } else {
                        while y > 0 && blank(y - 1) {
                            y -= 1;
                        }
                        while y > 0 && !blank(y - 1) {
                            y -= 1;
                        }
                        y = y.saturating_sub(1);
                    }
                }
                let x = if matches!(m, M::ParaF) && y == nl - 1 && !blank(y) { b.lines[y].len() } else { 0 };
                t((y, x), TK::Excl)
            }
            M::ScrTop | M::ScrMid | M::ScrBot => {
                let top = b.top;
                let th = self.text_h();
                let bot = (top + th).min(nl) - 1;
                let y = match m {
                    M::ScrTop => (top + n - 1).min(bot),
                    M::ScrBot => bot.saturating_sub(n - 1).max(top),
                    _ => (top + bot) / 2,
                };
                t((y, fnb(&b.lines[y])), TK::Line)
            }
            M::SearchN(rev) => {
                let re = self.search.take()?;
                let fwd = self.search_fwd != rev;
                let mut p = cur;
                let mut ok = true;
                for _ in 0..n {
                    match self.search_from(p, fwd, &re) {
                        Some((s, _)) => p = s,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                self.search = Some(re);
                self.show_hl = true;
                if !ok {
                    self.err(format!("pattern not found: {}", self.search_pat));
                    return None;
                }
                if p == cur {
                    self.info("only match");
                    return None;
                }
                self.last_jump = cur;
                if (fwd && p < cur) || (!fwd && p > cur) {
                    self.info("search wrapped");
                }
                t(p, TK::Excl)
            }
            M::Star(fwd) => {
                let w = self.word_under_cursor()?;
                let pat = format!("\\b{}\\b", regex_escape(&w));
                self.search = Some(Regex::new(&pat, false).ok()?);
                self.search_pat = pat;
                self.search_fwd = fwd;
                self.show_hl = true;
                // start from the beginning of the word so * doesn't land on itself
                self.eval_motion(M::SearchN(false), n, false, op)
            }
            M::Mark(c, exact) => {
                let p = match c {
                    '\'' | '`' => self.last_jump,
                    '<' => self.last_vis.map(|v| v.0.min(v.1))?,
                    '>' => self.last_vis.map(|v| v.0.max(v.1))?,
                    'a'..='z' => b.marks[c as usize - 'a' as usize]?,
                    _ => return None,
                };
                self.last_jump = cur;
                let p = (p.0.min(nl - 1), p.1);
                if exact {
                    t(p, TK::Excl)
                } else {
                    let l = &self.bb().lines[p.0];
                    t((p.0, fnb(l)), TK::Line)
                }
            }
            M::Obj(inner, o) => self.text_object(inner, o, n),
        }
    }

    // ---------- registers ----------
    pub fn set_reg(&mut self, r: char, text: String, line: bool, yank: bool) {
        if r == '_' {
            return;
        }
        let reg = Reg { text, line };
        if r.is_ascii_uppercase() {
            let e = self.regs.entry(r.to_ascii_lowercase()).or_default();
            if reg.line && !e.line && !e.text.is_empty() {
                e.text.push('\n');
            }
            e.text.push_str(&reg.text);
            e.line |= reg.line;
            let v = e.clone();
            self.regs.insert('"', v);
            return;
        }
        if r == '+' || r == '*' {
            self.osc52(&reg.text);
        }
        if yank {
            self.regs.insert('0', reg.clone());
        }
        if r != '"' {
            self.regs.insert(r, reg.clone());
        }
        self.regs.insert('"', reg);
    }

    pub fn osc52(&mut self, s: &str) {
        self.screen.osc52(s);
    }

    fn get_reg(&self, r: char) -> Option<Reg> {
        let r = if r == '+' || r == '*' { if self.regs.contains_key(&r) { r } else { '"' } } else { r.to_ascii_lowercase() };
        self.regs.get(&r).cloned()
    }

    // ---------- edit operations ----------
    pub fn delete_lines(&mut self, l1: usize, l2: usize) -> String {
        let b = self.b();
        let n = b.lines.len();
        let l2 = l2.min(n - 1);
        let text = b.lines[l1..=l2].join("\n") + "\n";
        if l2 + 1 < n {
            b.delete((l1, 0), (l2 + 1, 0));
        } else if l1 > 0 {
            let a = (l1 - 1, b.line_len(l1 - 1));
            let e = (l2, b.line_len(l2));
            b.delete(a, e);
        } else {
            let e = (l2, b.line_len(l2));
            b.delete((0, 0), e);
        }
        text
    }

    /// Reflow lines l1..=l2 to the text width, paragraph by paragraph. A
    /// paragraph is a run of lines sharing an indent and comment leader; blank
    /// lines (and bare leaders) are kept. Returns the last line of the result.
    fn format_lines(&mut self, l1: usize, l2: usize) -> usize {
        const LEADERS: [&str; 7] = ["///", "//!", "//", "#", "--", ";", ">"];
        let ts = self.opts.tabstop;
        let tw = self.opts.textwidth.max(10);
        let split = |l: &str| -> (String, String) {
            let ind = indent_of(l);
            let rest = &l[ind.len()..];
            let mut lead = ind.to_string();
            let mut body = rest;
            if let Some(m) = LEADERS.iter().find(|m| rest.starts_with(**m)) {
                lead.push_str(m);
                body = &rest[m.len()..];
                if body.starts_with(' ') {
                    lead.push(' ');
                    body = &body[1..];
                }
            }
            (lead, body.trim_end().to_string())
        };
        let old: Vec<String> = self.bb().lines[l1..=l2].to_vec();
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        while i < old.len() {
            let (lead, body) = split(&old[i]);
            if body.trim().is_empty() {
                out.push(old[i].trim_end().to_string());
                i += 1;
                continue;
            }
            let mut words: Vec<String> = body.split_whitespace().map(String::from).collect();
            // later lines may be indented differently (hanging indent): use the second line's
            let mut rest_lead = lead.clone();
            let mut j = i + 1;
            while j < old.len() {
                let (l2_, b2) = split(&old[j]);
                if b2.trim().is_empty() || (l2_.trim_end() != lead.trim_end() && !(j == i + 1 && l2_.trim_start() == lead.trim_start())) {
                    break;
                }
                if j == i + 1 {
                    rest_lead = l2_;
                }
                words.extend(b2.split_whitespace().map(String::from));
                j += 1;
            }
            let mut cur = lead.clone();
            let mut filled = false;
            for w in words {
                let width = disp_col(&cur, cur.len(), ts);
                let ww: usize = w.chars().map(char_width).sum();
                if filled && width + 1 + ww > tw {
                    out.push(std::mem::replace(&mut cur, rest_lead.clone()));
                    filled = false;
                }
                if filled {
                    cur.push(' ');
                }
                cur.push_str(&w);
                filled = true;
            }
            out.push(cur);
            i = j;
        }
        let n = out.len();
        let b = self.b();
        if out != old {
            let end = b.line_len(l2);
            b.delete((l1, 0), (l2, end));
            b.insert((l1, 0), &out.join("\n"));
        }
        l1 + n - 1
    }

    fn comment_lines(&mut self, l1: usize, l2: usize) {
        let lang = self.bb().lang;
        let (open, close) = if !lang.line_comment.is_empty() {
            (lang.line_comment.to_string(), String::new())
        } else if !lang.block.0.is_empty() {
            (lang.block.0.to_string(), lang.block.1.to_string())
        } else {
            ("#".to_string(), String::new())
        };
        let b = self.b();
        let nonblank: Vec<usize> = (l1..=l2).filter(|&i| !b.lines[i].trim().is_empty()).collect();
        if nonblank.is_empty() {
            return;
        }
        let all = nonblank.iter().all(|&i| b.lines[i].trim_start().starts_with(open.as_str()));
        let min_ind = nonblank.iter().map(|&i| fnb(&b.lines[i])).min().unwrap_or(0);
        for &i in &nonblank {
            let l = b.lines[i].clone();
            let ind = fnb(&l);
            if all {
                let mut e = ind + open.len();
                if l[e..].starts_with(' ') {
                    e += 1;
                }
                if !close.is_empty() {
                    let t = l.trim_end();
                    if t.ends_with(close.as_str()) && t.len() >= e + close.len() {
                        let mut cs = t.len() - close.len();
                        if l[..cs].ends_with(' ') && cs > e {
                            cs -= 1;
                        }
                        b.delete((i, cs), (i, l.len()));
                    }
                }
                b.delete((i, ind), (i, e));
            } else {
                if !close.is_empty() {
                    let ll = l.len();
                    b.insert((i, ll), &format!(" {}", close));
                }
                b.insert((i, min_ind), &format!("{} ", open));
            }
        }
    }

    pub fn indent_lines(&mut self, l1: usize, l2: usize, dedent: bool, times: usize) {
        let b = self.b();
        let unit = b.indent_unit();
        for i in l1..=l2.min(b.lines.len() - 1) {
            for _ in 0..times {
                if dedent {
                    let l = &b.lines[i];
                    let n = if l.starts_with('\t') { 1 } else { l.len() - l.trim_start_matches(' ').len() }.min(if l.starts_with('\t') { 1 } else { b.indent_w });
                    b.delete((i, 0), (i, n));
                } else if !b.lines[i].is_empty() {
                    b.insert((i, 0), &unit);
                }
            }
        }
    }

    fn clamp_pos(&self, p: Pos) -> Pos {
        let b = self.bb();
        let y = p.0.min(b.lines.len() - 1);
        let l = &b.lines[y];
        let mut x = p.1.min(l.len());
        while !l.is_char_boundary(x) {
            x -= 1;
        }
        (y, x)
    }

    fn apply_op(&mut self, op: Op, t: Target, reg: char) {
        let t = Target { a: self.clamp_pos(t.a), b: self.clamp_pos(t.b), kind: t.kind };
        let (mut a, mut z) = if t.a <= t.b { (t.a, t.b) } else { (t.b, t.a) };
        if t.kind == TK::Line {
            let (l1, l2) = (a.0, z.0);
            match op {
                Op::Del => {
                    let text = self.delete_lines(l1, l2);
                    self.set_reg(reg, text, true, false);
                    let y = l1.min(self.bb().lines.len() - 1);
                    let x = fnb(&self.bb().lines[y]);
                    self.set_cursor((y, x));
                }
                Op::Yank => {
                    let text = self.bb().lines[l1..=l2].join("\n") + "\n";
                    self.set_reg(reg, text, true, true);
                    if t.a > t.b || matches!(op, Op::Yank) && t.a.0 > l1 {
                        self.set_cursor((l1, self.cursor().1.min(self.bb().line_len(l1))));
                    }
                }
                Op::Change => {
                    let text = self.bb().lines[l1..=l2].join("\n") + "\n";
                    self.set_reg(reg, text, true, false);
                    if l2 > l1 {
                        let b = self.b();
                        let e = b.line_len(l2);
                        let s = b.line_len(l1);
                        b.delete((l1, s), (l2, e));
                    }
                    let ind = indent_of(&self.bb().lines[l1]).len();
                    let len = self.bb().line_len(l1);
                    self.b().delete((l1, ind), (l1, len));
                    self.set_cursor((l1, ind));
                    self.start_insert();
                }
                Op::Indent | Op::Dedent => {
                    self.indent_lines(l1, l2, op == Op::Dedent, 1);
                    let x = fnb(&self.bb().lines[l1]);
                    self.set_cursor((l1, x));
                }
                Op::Comment => {
                    self.comment_lines(l1, l2);
                    self.set_cursor((l1, self.cursor().1));
                }
                Op::Format | Op::FormatKeep => {
                    let keep = self.cursor();
                    let last = self.format_lines(l1, l2);
                    if op == Op::Format {
                        let x = fnb(&self.bb().lines[last]);
                        self.set_cursor((last, x));
                    } else {
                        self.set_cursor(self.clamp_pos(keep));
                    }
                }
                Op::Lower | Op::Upper | Op::Toggle => {
                    let e = self.bb().line_len(l2);
                    self.case_range((l1, 0), (l2, e), op);
                    self.set_cursor((l1, self.cursor().1));
                }
            }
            return;
        }
        if t.kind == TK::Incl {
            let l = &self.bb().lines[z.0];
            z.1 = next_boundary(l, z.1.min(l.len()));
        } else if z.1 == 0 && z.0 > a.0 && !matches!(op, Op::Yank) {
            // exclusive motion ending at column 0 stops at end of previous line
            z = (z.0 - 1, self.bb().line_len(z.0 - 1));
            if a.1 <= fnb(&self.bb().lines[a.0]) && op != Op::Change {
                return self.apply_op(op, Target { a, b: z, kind: TK::Line }, reg);
            }
        }
        if matches!(op, Op::Indent | Op::Dedent | Op::Comment | Op::Format | Op::FormatKeep) {
            return self.apply_op(op, Target { a, b: z, kind: TK::Line }, reg);
        }
        a.1 = a.1.min(self.bb().line_len(a.0));
        match op {
            Op::Del | Op::Change => {
                let text = self.b().delete(a, z);
                self.set_reg(reg, text, false, false);
                self.set_cursor(a);
                if op == Op::Change {
                    self.start_insert();
                }
            }
            Op::Yank => {
                let text = self.bb().text(a, z);
                self.set_reg(reg, text, false, true);
                self.set_cursor(a);
            }
            _ => {
                self.case_range(a, z, op);
                self.set_cursor(a);
            }
        }
    }

    fn case_range(&mut self, a: Pos, z: Pos, op: Op) {
        let s = self.bb().text(a, z);
        let t: String = match op {
            Op::Lower => s.to_lowercase(),
            Op::Upper => s.to_uppercase(),
            _ => s.chars().map(|c| if c.is_uppercase() { c.to_lowercase().next().unwrap_or(c) } else { c.to_uppercase().next().unwrap_or(c) }).collect(),
        };
        if t != s {
            self.b().delete(a, z);
            self.b().insert(a, &t);
        }
    }

    fn paste(&mut self, reg: char, before: bool, n: usize) {
        let Some(r) = self.get_reg(reg) else {
            self.err("register empty");
            return;
        };
        let (cy, cx) = self.cursor();
        if r.line {
            let mut text = r.text.repeat(n);
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let nl = self.bb().lines.len();
            let y = if before {
                self.b().insert((cy, 0), &text);
                cy
            } else if cy + 1 < nl {
                self.b().insert((cy + 1, 0), &text);
                cy + 1
            } else {
                let len = self.bb().line_len(cy);
                self.b().insert((cy, len), &format!("\n{}", &text[..text.len() - 1]));
                cy + 1
            };
            let x = fnb(&self.bb().lines[y]);
            self.set_cursor((y, x));
        } else {
            let text = r.text.repeat(n);
            let l = &self.bb().lines[cy];
            let p = if before || l.is_empty() { (cy, cx) } else { (cy, next_boundary(l, cx)) };
            let end = self.b().insert(p, &text);
            let l = &self.bb().lines[end.0];
            let x = if text.contains('\n') && end.1 == 0 { 0 } else { prev_boundary(l, end.1) };
            self.set_cursor((end.0, x));
        }
    }

    fn join_lines(&mut self, n: usize, spaces: bool) {
        let cy = self.cursor().0;
        let mut x = 0;
        for _ in 0..n.max(2) - 1 {
            let b = self.b();
            if cy + 1 >= b.lines.len() {
                break;
            }
            let cur_len = b.line_len(cy);
            let next = b.lines[cy + 1].clone();
            let lead = if spaces { fnb(&next) } else { 0 };
            b.delete((cy, cur_len), (cy + 1, lead));
            let rest = &next[lead..];
            if spaces && !rest.is_empty() && !rest.starts_with(')') && cur_len > 0 && !b.lines[cy].ends_with(' ') {
                b.insert((cy, cur_len), " ");
                x = cur_len;
            } else {
                x = cur_len;
            }
        }
        self.set_cursor((cy, x));
    }

    fn incr(&mut self, delta: i64) {
        let (cy, cx) = self.cursor();
        let l = self.bb().lines[cy].clone();
        let bytes = l.as_bytes();
        // find a number at or after the cursor
        let mut s = cx;
        while s > 0 && bytes[s - 1].is_ascii_digit() && bytes.get(s).map_or(false, |b| b.is_ascii_digit()) {
            s -= 1;
        }
        while s < bytes.len() && !bytes[s].is_ascii_digit() {
            s += 1;
        }
        if s >= bytes.len() {
            return;
        }
        while s > 0 && bytes[s - 1].is_ascii_digit() {
            s -= 1;
        }
        let neg = s > 0 && bytes[s - 1] == b'-';
        let mut e = s;
        while e < bytes.len() && bytes[e].is_ascii_digit() {
            e += 1;
        }
        let start = if neg { s - 1 } else { s };
        let v: i64 = l[start..e].parse().unwrap_or(0);
        let nv = (v + delta).to_string();
        self.b().delete((cy, start), (cy, e));
        self.b().insert((cy, start), &nv);
        self.set_cursor((cy, start + nv.len() - 1));
    }

    pub fn start_insert(&mut self) {
        self.mode = Mode::Insert;
        self.comp = None;
    }

    fn open_line(&mut self, above: bool) {
        let (cy, _) = self.cursor();
        let l = self.bb().lines[cy].clone();
        let mut ind = indent_of(&l).to_string();
        if !above {
            let t = l.trim_end();
            if t.ends_with('{') || t.ends_with('[') || t.ends_with('(') || (self.bb().lang.flags & crate::syntax::F_INDENT_COLON != 0 && t.ends_with(':')) {
                ind.push_str(&self.bb().indent_unit());
            }
        }
        if above {
            self.b().insert((cy, 0), &format!("{}\n", ind));
            self.set_cursor((cy, ind.len()));
        } else {
            let len = l.len();
            self.b().insert((cy, len), &format!("\n{}", ind));
            self.set_cursor((cy + 1, ind.len()));
        }
        self.start_insert();
    }

    // ---------- mouse ----------

    /// Scroll the view by `d` lines, moving the cursor only as far as needed
    /// to stay outside the scrolloff margin (so `scroll()` keeps the view).
    pub fn scroll_view(&mut self, d: isize) {
        let th = self.text_h();
        let so = 3.min(th.saturating_sub(1) / 2);
        let ts = self.opts.tabstop;
        let b = self.b();
        let last = b.lines.len() - 1;
        b.top = (b.top as isize + d).clamp(0, last as isize) as usize;
        let lo = if b.top == 0 { 0 } else { b.top + so };
        let hi = (b.top + th).saturating_sub(1 + so).max(lo);
        let cy = b.cy.clamp(lo.min(last), hi.min(last));
        if cy != b.cy {
            b.cy = cy;
            b.cx = byte_at_disp(&b.lines[cy], b.want_x, ts);
        }
        self.clamp();
    }

    /// The buffer position under a screen cell in the text area.
    fn mouse_pos(&self, x: usize, y: usize) -> Option<Pos> {
        if y >= self.text_h() {
            return None;
        }
        let b = self.bb();
        let ln = (b.top + y).min(b.lines.len() - 1);
        let disp = (x.saturating_sub(self.gutter_w()) + b.left) as usize;
        Some((ln, byte_at_disp(&b.lines[ln], disp, self.opts.tabstop)))
    }

    fn mouse(&mut self, m: Mouse) {
        if matches!(self.mode, Mode::Cmd(_)) {
            return;
        }
        match m.kind {
            MouseKind::WheelUp => self.scroll_view(-3),
            MouseKind::WheelDown => self.scroll_view(3),
            MouseKind::Press(0) => {
                let Some(p) = self.mouse_pos(m.x, m.y) else { return };
                self.pending.clear();
                let n = match self.last_click {
                    Some((t, q, n)) if q.0 == p.0 && (n == 1 && q == p || n > 1) && t.elapsed() < std::time::Duration::from_millis(400) => n % 3 + 1,
                    _ => 1,
                };
                self.last_click = Some((std::time::Instant::now(), p, n));
                let insert = self.mode == Mode::Insert;
                if insert {
                    self.handle_key(Key::Esc);
                }
                self.mode = Mode::Normal;
                self.set_cursor(p);
                self.drag_from = None;
                match n {
                    // double click: select the word
                    2 => {
                        if let Some(t) = self.text_object(true, 'w', 1) {
                            let line = &self.bb().lines[t.b.0];
                            let end = if t.b.1 > t.a.1 { prev_boundary(line, t.b.1) } else { t.b.1 };
                            self.vstart = t.a;
                            self.mode = Mode::Visual;
                            self.set_cursor((t.b.0, end));
                        }
                    }
                    // triple click: select the line
                    3 => {
                        self.vstart = p;
                        self.mode = Mode::VisualLine;
                    }
                    _ => {
                        self.drag_from = Some(p);
                        if insert {
                            self.start_insert();
                        } else {
                            self.clamp();
                        }
                    }
                }
            }
            MouseKind::Drag(0) => {
                let Some(from) = self.drag_from else { return };
                let y = m.y.min(self.text_h() - 1);
                let Some(p) = self.mouse_pos(m.x, y) else { return };
                if p == from && self.mode != Mode::Visual {
                    return;
                }
                if self.mode == Mode::Insert {
                    self.handle_key(Key::Esc);
                }
                if !matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                    self.vstart = from;
                    self.mode = Mode::Visual;
                }
                self.set_cursor(p);
            }
            MouseKind::Release => self.drag_from = None,
            _ => {}
        }
    }

    // ---------- key dispatch ----------
    pub fn handle_key(&mut self, k: Key) {
        if let Key::Mouse(m) = k {
            if !self.msg_lines.is_empty() {
                if matches!(m.kind, MouseKind::Press(_)) {
                    self.msg_lines.clear();
                    self.screen.invalidate();
                }
                return;
            }
            if self.preview.is_some() {
                return self.preview_mouse(m);
            }
            if self.picker.is_some() {
                return self.picker_key(k);
            }
            return self.mouse(m);
        }
        if k == Key::Alt('\n') {
            return self.handle_key(Key::Enter);
        }
        if let Key::Alt(c) = k {
            if self.mode != Mode::Normal || !self.pending.is_empty() {
                self.handle_key(Key::Esc);
            }
            self.handle_key(Key::Char(c));
            return;
        }
        if let Some((_, v)) = &mut self.mac_rec {
            v.push(k.clone());
        }
        if !self.msg_lines.is_empty() {
            self.msg_lines.clear();
            self.screen.invalidate();
            if matches!(k, Key::Esc | Key::Enter | Key::Char(' ') | Key::Char('q')) {
                return;
            }
        }
        if self.preview.is_some() {
            return self.preview_key(k);
        }
        if self.picker.is_some() {
            self.picker_key(k);
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(k),
            Mode::Insert => {
                if let Some(r) = &mut self.dot_rec {
                    r.push(k.clone());
                }
                self.insert_key(k)
            }
            Mode::Visual | Mode::VisualLine => self.visual_key(k),
            Mode::Cmd(_) => self.cmd_key(k),
        }
    }

    fn normal_key(&mut self, k: Key) {
        if self.pending.is_empty() {
            if !matches!(k, Key::Char(':')) {
                self.msg.clear();
            }
            match k {
                Key::Esc => {
                    self.show_hl = false;
                    return;
                }
                Key::Ctrl('c') => {
                    self.show_hl = false;
                    self.info("type :q to quit");
                    return;
                }
                _ => {}
            }
        } else if k == Key::Esc {
            self.pending.clear();
            return;
        }
        self.clamp();
        self.pending.push(k);
        let keys = std::mem::take(&mut self.pending);
        let ver = (self.cur, self.bb().version);
        let cur = self.cursor();
        self.b().mark_start(cur);
        match self.run_normal(&keys) {
            P::Pending => {
                self.pending = keys;
                return;
            }
            P::Invalid | P::Ok(_) => {}
        }
        if !self.replaying {
            let changed = ver != (self.cur, self.bb().version);
            let first = keys.iter().find(|k| !matches!(k, Key::Char('0'..='9')));
            let repeatable = !matches!(first, Some(Key::Char('u' | '.' | '@' | 'q' | ':' | '/' | '?')) | Some(Key::Ctrl('r')));
            if self.mode == Mode::Insert && repeatable {
                self.dot_rec = Some(keys);
            } else if changed && repeatable && ver.0 == self.cur {
                self.dot = keys;
            }
        }
        if self.mode == Mode::Normal {
            let c = self.cursor();
            self.b().commit(c);
            self.clamp();
        }
    }

    fn run_normal(&mut self, keys: &[Key]) -> P<()> {
        let mut i = 0;
        let mut reg = '"';
        if keys[0] == Key::Char('"') {
            match keys.get(1) {
                None => return P::Pending,
                Some(Key::Char(c)) => reg = *c,
                _ => return P::Invalid,
            }
            i = 2;
        }
        let (cnt, j) = parse_count(keys, i);
        i = j;
        let Some(k) = keys.get(i) else { return P::Pending };
        let n = cnt.unwrap_or(1);
        let has_count = cnt.is_some();
        let next = keys.get(i + 1).and_then(kc);
        let more = keys.len() > i + 1;

        // leader (space) mappings
        if *k == Key::Char(' ') {
            if !more {
                return P::Pending;
            }
            match next {
                Some('f') => self.open_files_picker(),
                Some('b') => self.open_buffer_picker(),
                Some('d') => self.open_diag_picker(),
                Some('s') => self.open_symbol_picker(),
                Some('/') => {
                    self.mode = Mode::Cmd(':');
                    self.cmdline = "grep ".into();
                }
                Some('c') => self.run_check(true),
                Some('w') => self.ex("w"),
                Some('y') | Some('p') | Some('P') => {
                    let mut nk = vec![Key::Char('"'), Key::Char('+')];
                    nk.extend_from_slice(&keys[i + 1..]);
                    return self.run_normal(&nk);
                }
                _ => return P::Invalid,
            }
            return P::Ok(());
        }

        // operators
        let (op, oplen) = match (k, next) {
            (Key::Char('d'), _) => (Some(Op::Del), 1),
            (Key::Char('c'), _) => (Some(Op::Change), 1),
            (Key::Char('y'), _) => (Some(Op::Yank), 1),
            (Key::Char('>'), _) => (Some(Op::Indent), 1),
            (Key::Char('<'), _) => (Some(Op::Dedent), 1),
            (Key::Char('g'), Some('~')) => (Some(Op::Toggle), 2),
            (Key::Char('g'), Some('u')) => (Some(Op::Lower), 2),
            (Key::Char('g'), Some('U')) => (Some(Op::Upper), 2),
            (Key::Char('g'), Some('c')) => (Some(Op::Comment), 2),
            (Key::Char('g'), Some('q')) => (Some(Op::Format), 2),
            (Key::Char('g'), Some('w')) => (Some(Op::FormatKeep), 2),
            _ => (None, 0),
        };
        if let Some(op) = op {
            let j = i + oplen;
            let (c2, j) = parse_count(keys, j);
            let Some(mk) = keys.get(j) else { return P::Pending };
            let total = n * c2.unwrap_or(1);
            let has = has_count || c2.is_some();
            let last = keys[i + oplen - 1].clone();
            let doubled = *mk == last || (oplen == 2 && *mk == Key::Char('g') && keys.get(j + 1) == Some(&last));
            if oplen == 2 && *mk == Key::Char('g') && keys.len() == j + 1 {
                return P::Pending;
            }
            if doubled {
                let cy = self.cursor().0;
                let end = (cy + total - 1).min(self.bb().lines.len() - 1);
                self.apply_op(op, Target { a: (cy, self.cursor().1), b: (end, 0), kind: TK::Line }, reg);
                return P::Ok(());
            }
            return match parse_motion(keys, j, true) {
                P::Ok((m, _)) => {
                    // cw behaves like ce
                    let m = match (op, m) {
                        (Op::Change, M::WordF(big)) if !self.ch(self.cursor()).is_whitespace() => M::WordE(big),
                        _ => m,
                    };
                    if let Some(t) = self.eval_motion(m, total, has, true) {
                        self.apply_op(op, t, reg);
                    }
                    P::Ok(())
                }
                P::Pending => P::Pending,
                P::Invalid => P::Invalid,
            };
        }

        match parse_motion(keys, i, false) {
            P::Ok((m, _)) => {
                if let Some(t) = self.eval_motion(m, n, has_count, false) {
                    let want = self.bb().want_x;
                    let keep_want = matches!(m, M::Up | M::Down);
                    if keep_want {
                        let ts = self.opts.tabstop;
                        let b = self.b();
                        b.cy = t.b.0;
                        b.cx = byte_at_disp(&b.lines[b.cy], want, ts);
                    } else {
                        self.set_cursor(t.b);
                        if matches!(m, M::LineEnd) {
                            self.b().want_x = usize::MAX;
                        }
                    }
                }
                return P::Ok(());
            }
            P::Pending => return P::Pending,
            P::Invalid => {}
        }

        let (cy, cx) = self.cursor();
        let need = |c: Option<char>| if c.is_none() && !more { P::Pending } else { P::Invalid };
        // translate shorthand commands into operator forms
        let alias: Option<&str> = match k {
            Key::Char('x') | Key::Delete => Some("dl"),
            Key::Char('X') => Some("dh"),
            Key::Char('D') => Some("d$"),
            Key::Char('C') => Some("c$"),
            Key::Char('s') => Some("cl"),
            Key::Char('S') => Some("cc"),
            Key::Char('Y') => Some("yy"),
            _ => None,
        };
        if let Some(a) = alias {
            if matches!(k, Key::Char('x') | Key::Delete) && self.bb().lines[cy].is_empty() {
                return P::Ok(());
            }
            let mut nk: Vec<Key> = Vec::new();
            if reg != '"' {
                nk.push(Key::Char('"'));
                nk.push(Key::Char(reg));
            }
            if let Some(c) = cnt {
                nk.extend(c.to_string().chars().map(Key::Char));
            }
            nk.extend(a.chars().map(Key::Char));
            return self.run_normal(&nk);
        }

        match k {
            Key::Char('i') => self.start_insert(),
            Key::Char('a') => {
                let l = &self.bb().lines[cy];
                let x = if l.is_empty() { 0 } else { next_boundary(l, cx) };
                self.b().cx = x;
                self.start_insert();
            }
            Key::Char('I') => {
                let x = fnb(&self.bb().lines[cy]);
                self.b().cx = x;
                self.start_insert();
            }
            Key::Char('A') => {
                let x = self.bb().line_len(cy);
                self.b().cx = x;
                self.start_insert();
            }
            Key::Char('o') => self.open_line(false),
            Key::Char('O') => self.open_line(true),
            Key::Char('v') => {
                self.vstart = (cy, cx);
                self.mode = Mode::Visual;
            }
            Key::Char('V') => {
                self.vstart = (cy, cx);
                self.mode = Mode::VisualLine;
            }
            Key::Char('p') => self.paste(reg, false, n),
            Key::Char('P') => self.paste(reg, true, n),
            Key::Char('u') => match self.b().undo() {
                Some(p) => {
                    self.set_cursor(p);
                    self.info("undo");
                }
                None => self.info("already at oldest change"),
            },
            Key::Ctrl('r') => match self.b().redo() {
                Some(p) => {
                    self.set_cursor(p);
                    self.info("redo");
                }
                None => self.info("already at newest change"),
            },
            Key::Char('.') => {
                let keys = self.dot.clone();
                if keys.is_empty() {
                    return P::Ok(());
                }
                self.replaying = true;
                for _ in 0..n {
                    for k in keys.iter().cloned() {
                        self.handle_key(k);
                    }
                    if self.mode == Mode::Insert {
                        self.handle_key(Key::Esc);
                    }
                }
                self.replaying = false;
            }
            Key::Char('J') => self.join_lines(n, true),
            Key::Char('~') => {
                let l = &self.bb().lines[cy];
                if !l.is_empty() {
                    let mut e = cx;
                    for _ in 0..n {
                        e = next_boundary(l, e);
                    }
                    self.case_range((cy, cx), (cy, e), Op::Toggle);
                    let l = &self.bb().lines[cy];
                    self.set_cursor((cy, e.min(prev_boundary(l, l.len()))));
                }
            }
            Key::Char('r') => {
                let c = match keys.get(i + 1) {
                    None => return P::Pending,
                    Some(Key::Char(c)) => *c,
                    Some(Key::Enter) => '\n',
                    Some(Key::Tab) => '\t',
                    _ => return P::Invalid,
                };
                let l = &self.bb().lines[cy];
                let mut e = cx;
                for _ in 0..n {
                    if e >= l.len() {
                        return P::Ok(());
                    }
                    e = next_boundary(l, e);
                }
                self.b().delete((cy, cx), (cy, e));
                if c == '\n' {
                    self.b().insert((cy, cx), "\n");
                    self.set_cursor((cy + 1, 0));
                } else {
                    let s: String = std::iter::repeat(c).take(n).collect();
                    self.b().insert((cy, cx), &s);
                    self.set_cursor((cy, cx + s.len() - c.len_utf8()));
                }
            }
            Key::Char('m') => match next {
                Some(c @ 'a'..='z') => self.b().marks[c as usize - 'a' as usize] = Some((cy, cx)),
                _ => return need(next),
            },
            Key::Char('q') => {
                if let Some((r, mut v)) = self.mac_rec.take() {
                    v.pop(); // the closing 'q'
                    self.macros.insert(r, v);
                    self.info(format!("recorded @{}", r));
                } else {
                    match next {
                        Some(c) if c.is_ascii_alphanumeric() => {
                            self.mac_rec = Some((c, Vec::new()));
                            self.info(format!("recording @{}", c));
                        }
                        _ => return need(next),
                    }
                }
            }
            Key::Char('@') => {
                let r = match next {
                    Some('@') => self.last_macro,
                    Some(c) => c,
                    None => return need(next),
                };
                self.last_macro = r;
                if let Some(m) = self.macros.get(&r).cloned() {
                    for _ in 0..n {
                        for k in m.iter().cloned() {
                            self.handle_key(k);
                        }
                    }
                }
            }
            Key::Char(':') => {
                self.mode = Mode::Cmd(':');
                self.cmdline = if has_count { format!(".,.+{}", n - 1) } else { String::new() };
                self.hist_idx = self.hist.len();
            }
            Key::Char('/') | Key::Char('?') => {
                let c = kc(k).unwrap();
                self.mode = Mode::Cmd(c);
                self.cmdline.clear();
                self.search_origin = ((cy, cx), self.bb().top);
                self.hist_idx = self.hist.len();
            }
            Key::Ctrl('d') | Key::Ctrl('u') | Key::Ctrl('f') | Key::Ctrl('b') | Key::PageDown | Key::PageUp => {
                let th = self.text_h();
                let amt = match k {
                    Key::Ctrl('d') | Key::Ctrl('u') => th / 2,
                    _ => th.saturating_sub(2),
                } as isize;
                let d = if matches!(k, Key::Ctrl('u') | Key::Ctrl('b') | Key::PageUp) { -amt } else { amt } * n as isize;
                let nl = self.bb().lines.len() as isize;
                let b = self.b();
                b.top = (b.top as isize + d).clamp(0, (nl - 1).max(0)) as usize;
                b.cy = (b.cy as isize + d).clamp(0, nl - 1) as usize;
                let x = fnb(&b.lines[b.cy]);
                self.set_cursor((self.bb().cy, x));
            }
            Key::Ctrl('e') => self.scroll_view(n as isize),
            Key::Ctrl('y') => self.scroll_view(-(n as isize)),
            Key::Ctrl('a') => self.incr(n as i64),
            Key::Ctrl('x') => self.incr(-(n as i64)),
            Key::Ctrl('s') => self.ex("w"),
            Key::Ctrl('g') => {
                let b = self.bb();
                let s = format!("\"{}\"{} {} lines --{}%-- {} indent={}{}", b.name, if b.dirty { " [+]" } else { "" }, b.lines.len(), (cy + 1) * 100 / b.lines.len(), b.lang.name, b.indent_w, if b.expand_tab { " spaces" } else { " tabs" });
                self.info(s);
            }
            Key::Ctrl('^') | Key::Ctrl('6') => {
                if self.alt < self.bufs.len() && self.alt != self.cur {
                    let a = self.alt;
                    self.switch_buf(a);
                }
            }
            Key::Ctrl('z') => self.suspend = true,
            Key::Ctrl('l') => self.screen.invalidate(),
            Key::Char('K') => self.show_line_diags(),
            Key::Char('z') => {
                let th = self.text_h();
                let b = self.b();
                match next {
                    Some('z') | Some('.') => b.top = b.cy.saturating_sub(th / 2),
                    Some('t') | Some('\r') => b.top = b.cy,
                    Some('b') | Some('-') => b.top = (b.cy + 1).saturating_sub(th),
                    _ => return need(next),
                }
            }
            Key::Char('g') => match next {
                Some('d') => self.goto_definition(),
                Some('f') => self.goto_file(),
                Some('v') => {
                    if let Some((a, z, m)) = self.last_vis {
                        self.vstart = self.clamp_pos(a);
                        self.set_cursor(z);
                        self.mode = m;
                    }
                }
                Some('J') => self.join_lines(n, false),
                _ => return need(next),
            },
            Key::Char(']') | Key::Char('[') => {
                let fwd = *k == Key::Char(']');
                match next {
                    Some('d') | Some('e') => self.jump_diag(fwd, next == Some('e')),
                    Some('q') => self.jump_qf(fwd),
                    Some('b') => {
                        let nb = self.bufs.len();
                        let i = if fwd { (self.cur + 1) % nb } else { (self.cur + nb - 1) % nb };
                        self.switch_buf(i);
                    }
                    _ => return need(next),
                }
            }
            _ => return P::Invalid,
        }
        P::Ok(())
    }

    // ---------- visual mode ----------
    pub fn vis_range(&self) -> Target {
        let c = self.cursor();
        let v = self.clamp_pos(self.vstart);
        let (a, z) = if v <= c { (v, c) } else { (c, v) };
        Target { a, b: z, kind: if self.mode == Mode::VisualLine { TK::Line } else { TK::Incl } }
    }

    fn visual_key(&mut self, k: Key) {
        self.pending.push(k);
        let keys = std::mem::take(&mut self.pending);
        let cur = self.cursor();
        self.b().mark_start(cur);
        let mut i = 0;
        let mut reg = '"';
        if keys[0] == Key::Char('"') {
            match keys.get(1) {
                None => {
                    self.pending = keys;
                    return;
                }
                Some(Key::Char(c)) => reg = *c,
                _ => return,
            }
            i = 2;
        }
        if keys.get(i) == Some(&Key::Char(' ')) {
            match keys.get(i + 1) {
                None => {
                    self.pending = keys;
                    return;
                }
                Some(Key::Char('y')) => {
                    reg = '+';
                    i += 1;
                }
                _ => return,
            }
        }
        let (cnt, j) = parse_count(&keys, i);
        i = j;
        let Some(k) = keys.get(i).cloned() else {
            self.pending = keys;
            return;
        };
        let n = cnt.unwrap_or(1);
        let next = keys.get(i + 1).and_then(kc);
        let t = self.vis_range();
        let exit = |s: &mut Self| {
            s.last_vis = Some((s.vstart, s.cursor(), s.mode));
            s.mode = Mode::Normal;
        };
        let op = match k {
            Key::Char('d') | Key::Char('x') | Key::Delete => Some(Op::Del),
            Key::Char('c') | Key::Char('s') => Some(Op::Change),
            Key::Char('y') => Some(Op::Yank),
            Key::Char('>') => Some(Op::Indent),
            Key::Char('<') => Some(Op::Dedent),
            Key::Char('~') => Some(Op::Toggle),
            Key::Char('u') => Some(Op::Lower),
            Key::Char('U') => Some(Op::Upper),
            Key::Char('g') => match next {
                None => {
                    self.pending = keys;
                    return;
                }
                Some('c') => Some(Op::Comment),
                Some('q') => Some(Op::Format),
                Some('w') => Some(Op::FormatKeep),
                Some('J') => {
                    exit(self);
                    self.set_cursor(t.a);
                    self.join_lines(t.b.0 - t.a.0 + 1, false);
                    self.after_change();
                    return;
                }
                _ => None,
            },
            Key::Char('D') | Key::Char('X') => {
                exit(self);
                self.apply_op(Op::Del, Target { kind: TK::Line, ..t }, reg);
                self.after_change();
                return;
            }
            Key::Char('C') | Key::Char('S') | Key::Char('R') => {
                exit(self);
                self.apply_op(Op::Change, Target { kind: TK::Line, ..t }, reg);
                return;
            }
            _ => None,
        };
        if let Some(op) = op {
            // `.` redoes this on the same amount of text: as many characters
            // on one line, else as many lines
            let ver = (self.cur, self.bb().version);
            let mut again = if self.mode == Mode::Visual && t.a.0 == t.b.0 {
                let l = &self.bb().lines[t.a.0];
                let e = next_boundary(l, t.b.1.min(l.len()));
                let chars = l[t.a.1.min(e)..e].chars().count().max(1);
                std::iter::once(Key::Char('v')).chain(std::iter::repeat(Key::Char('l')).take(chars - 1)).collect::<Vec<_>>()
            } else {
                std::iter::once(Key::Char('V')).chain(std::iter::repeat(Key::Char('j')).take(t.b.0 - t.a.0)).collect::<Vec<_>>()
            };
            again.extend(keys.iter().cloned());
            exit(self);
            if matches!(op, Op::Indent | Op::Dedent) {
                self.indent_lines(t.a.0, t.b.0, op == Op::Dedent, n);
                let x = fnb(&self.bb().lines[t.a.0]);
                self.set_cursor((t.a.0, x));
            } else {
                self.apply_op(op, t, reg);
            }
            if !self.replaying {
                if self.mode == Mode::Insert {
                    self.dot_rec = Some(again);
                } else if ver != (self.cur, self.bb().version) {
                    self.dot = again;
                }
            }
            if self.mode == Mode::Normal {
                self.after_change();
            }
            return;
        }
        match k {
            Key::Esc | Key::Ctrl('c') => exit(self),
            Key::Char('v') | Key::Char('V') => {
                let m = if k == Key::Char('v') { Mode::Visual } else { Mode::VisualLine };
                if self.mode == m {
                    exit(self);
                } else {
                    self.mode = m;
                }
            }
            Key::Char('o') => {
                let c = self.cursor();
                let v = self.vstart;
                self.vstart = c;
                self.set_cursor(v);
            }
            Key::Char('J') => {
                exit(self);
                self.set_cursor(t.a);
                self.join_lines(t.b.0 - t.a.0 + 1, true);
                self.after_change();
            }
            Key::Char('r') => {
                let Some(c) = next else {
                    if keys.len() == i + 1 {
                        self.pending = keys;
                    }
                    return;
                };
                exit(self);
                let (a, mut z) = (t.a, t.b);
                if t.kind == TK::Line {
                    z = (z.0, self.bb().line_len(z.0));
                } else {
                    z.1 = next_boundary(&self.bb().lines[z.0], z.1);
                }
                let s = self.bb().text(a, z);
                let r: String = s.chars().map(|x| if x == '\n' { x } else { c }).collect();
                self.b().delete(a, z);
                self.b().insert(a, &r);
                self.set_cursor(a);
                self.after_change();
            }
            Key::Char('p') | Key::Char('P') => {
                let Some(r) = self.get_reg(reg) else { return exit(self) };
                exit(self);
                self.apply_op(Op::Del, t, '_');
                self.regs.insert('"', r.clone());
                let before = t.kind != TK::Line && !(self.cursor().1 >= self.bb().line_len(self.cursor().0) && self.cursor().1 > 0 && t.a.1 >= self.bb().line_len(t.a.0));
                if r.line && t.kind != TK::Line {
                    let (cy, cx) = self.cursor();
                    self.b().insert((cy, cx), "\n");
                    self.set_cursor((cy + 1, 0));
                    self.paste(reg, true, 1);
                } else if t.kind == TK::Line && !r.line {
                    let cy = self.cursor().0;
                    self.b().insert((cy, 0), "\n");
                    self.set_cursor((cy, 0));
                    self.paste(reg, true, 1);
                } else {
                    self.paste(reg, before || t.kind == TK::Line, 1);
                }
                self.after_change();
            }
            Key::Char(':') => {
                exit(self);
                self.mode = Mode::Cmd(':');
                self.cmdline = "'<,'>".into();
            }
            Key::Char('I') => {
                exit(self);
                self.set_cursor((t.a.0, fnb(&self.bb().lines[t.a.0])));
                self.start_insert();
            }
            Key::Char('A') => {
                exit(self);
                let l = self.bb().line_len(t.b.0);
                self.set_cursor((t.b.0, l));
                self.start_insert();
            }
            _ => match parse_motion(&keys, i, true) {
                P::Ok((m, _)) => {
                    if let M::Obj(..) = m {
                        if let Some(r) = self.eval_motion(m, n, cnt.is_some(), true) {
                            if r.kind == TK::Line && self.mode == Mode::Visual {
                                self.mode = Mode::VisualLine;
                            }
                            self.vstart = r.a;
                            let z = if r.kind == TK::Line { r.b } else { (r.b.0, prev_boundary(&self.bb().lines[r.b.0], r.b.1)) };
                            self.set_cursor(z);
                        }
                    } else if let Some(r) = self.eval_motion(m, n, cnt.is_some(), false) {
                        let keep_want = matches!(m, M::Up | M::Down);
                        if keep_want {
                            let want = self.bb().want_x;
                            let ts = self.opts.tabstop;
                            let b = self.b();
                            b.cy = r.b.0;
                            b.cx = byte_at_disp(&b.lines[b.cy], want, ts);
                        } else {
                            self.set_cursor(r.b);
                        }
                    }
                }
                P::Pending => self.pending = keys,
                P::Invalid => {}
            },
        }
    }

    fn after_change(&mut self) {
        let c = self.cursor();
        self.b().commit(c);
        self.clamp();
    }

    // ---------- insert mode ----------
    fn insert_text(&mut self, s: &str) {
        let (cy, cx) = self.cursor();
        let end = self.b().insert((cy, cx), s);
        self.set_cursor(end);
    }

    fn insert_key(&mut self, k: Key) {
        if self.ins_reg_pending {
            self.ins_reg_pending = false;
            if let Key::Char(c) = k {
                if let Some(r) = self.get_reg(c) {
                    self.insert_text(&r.text);
                }
            }
            return;
        }
        if self.comp.is_some() {
            let comp = self.comp.as_mut().unwrap();
            let n = comp.items.len();
            match k {
                Key::Tab | Key::Ctrl('n') | Key::Down => {
                    comp.sel = Some(comp.sel.map_or(0, |s| (s + 1) % n));
                    return;
                }
                Key::BackTab | Key::Ctrl('p') | Key::Up => {
                    comp.sel = Some(comp.sel.map_or(n - 1, |s| (s + n - 1) % n));
                    return;
                }
                Key::Enter if comp.sel.is_some() => {
                    self.accept_completion();
                    return;
                }
                Key::Ctrl('e') => {
                    self.comp = None;
                    return;
                }
                _ => {}
            }
        }
        self.clamp();
        let (cy, cx) = self.cursor();
        match k {
            Key::Esc | Key::Ctrl('c') => {
                self.comp = None;
                self.sym_cache = None;
                self.mode = Mode::Normal;
                // drop auto-indent left on an otherwise empty line
                let l = &self.bb().lines[cy];
                if !l.is_empty() && l.trim().is_empty() && cx == l.len() {
                    let len = l.len();
                    self.b().delete((cy, 0), (cy, len));
                }
                let c = self.cursor();
                self.b().commit(c);
                let l = &self.bb().lines[cy];
                let x = prev_boundary(l, c.1.min(l.len()));
                self.set_cursor((cy, x));
                if let Some(r) = self.dot_rec.take() {
                    if !self.replaying {
                        self.dot = r;
                    }
                }
                return;
            }
            Key::Char(c) => {
                // auto-dedent closing brackets typed on a blank line
                if matches!(c, '}' | ')' | ']') {
                    let l = &self.bb().lines[cy];
                    if !l.is_empty() && l[..cx].trim().is_empty() {
                        let unit = self.bb().indent_unit();
                        if l[..cx].ends_with(&unit) {
                            self.b().delete((cy, cx - unit.len()), (cy, cx));
                            self.set_cursor((cy, cx - unit.len()));
                        }
                    }
                }
                let mut b = [0u8; 4];
                self.insert_text(c.encode_utf8(&mut b));
                if self.opts.autocomplete && (is_word(c) || c == '/' || c == '.') {
                    self.update_completion(false);
                } else {
                    self.comp = None;
                }
                return;
            }
            Key::Paste(s) => {
                self.insert_text(&s);
            }
            Key::Enter => {
                let l = self.bb().lines[cy].clone();
                let before = l[..cx].trim_end();
                let after = &l[cx..];
                let mut ind = indent_of(&l).to_string();
                if ind.len() > cx {
                    ind.truncate(cx);
                }
                let unit = self.bb().indent_unit();
                let opener = before.ends_with('{') || before.ends_with('[') || before.ends_with('(') || (self.bb().lang.flags & crate::syntax::F_INDENT_COLON != 0 && before.ends_with(':'));
                let closer = after.trim_start().starts_with(['}', ']', ')']);
                // strip trailing whitespace of current line, leading whitespace of the rest
                let lead = after.len() - after.trim_start().len();
                if lead > 0 {
                    self.b().delete((cy, cx), (cy, cx + lead));
                }
                let tw = cx - (l[..cx].len() - l[..cx].trim_end().len());
                if l[..cx].trim().is_empty() {
                    self.b().delete((cy, 0), (cy, cx));
                    self.set_cursor((cy, 0));
                } else if tw < cx {
                    self.b().delete((cy, tw), (cy, cx));
                    self.set_cursor((cy, tw));
                }
                if opener && closer {
                    let s = format!("\n{}{}\n{}", ind, unit, ind);
                    self.insert_text(&s);
                    let y = self.cursor().0 - 1;
                    self.set_cursor((y, ind.len() + unit.len()));
                } else if opener {
                    self.insert_text(&format!("\n{}{}", ind, unit));
                } else {
                    self.insert_text(&format!("\n{}", ind));
                }
            }
            Key::Backspace | Key::Ctrl('h') => {
                if cx > 0 {
                    let l = &self.bb().lines[cy];
                    let b = self.bb();
                    let s = if b.expand_tab && l[..cx].bytes().all(|c| c == b' ') {
                        let w = b.indent_w.max(1);
                        cx - ((cx - 1) % w + 1)
                    } else {
                        prev_boundary(l, cx)
                    };
                    self.b().delete((cy, s), (cy, cx));
                    self.set_cursor((cy, s));
                } else if cy > 0 {
                    let pl = self.bb().line_len(cy - 1);
                    self.b().delete((cy - 1, pl), (cy, 0));
                    self.set_cursor((cy - 1, pl));
                }
                if self.comp.is_some() {
                    self.update_completion(false);
                }
                return;
            }
            Key::Delete => {
                let len = self.bb().line_len(cy);
                if cx < len {
                    let e = next_boundary(&self.bb().lines[cy], cx);
                    self.b().delete((cy, cx), (cy, e));
                } else if cy + 1 < self.bb().lines.len() {
                    self.b().delete((cy, cx), (cy + 1, 0));
                }
            }
            Key::Tab => {
                let b = self.bb();
                let s = if b.expand_tab {
                    let d = disp_col(&b.lines[cy], cx, self.opts.tabstop);
                    " ".repeat(b.indent_w - d % b.indent_w.max(1))
                } else {
                    "\t".into()
                };
                self.insert_text(&s);
            }
            Key::Ctrl('w') => {
                let s = self.word_back((cy, cx), false);
                let s = if s.0 < cy { (cy, 0) } else { s };
                if s == (cy, cx) && cx == 0 && cy > 0 {
                    let pl = self.bb().line_len(cy - 1);
                    self.b().delete((cy - 1, pl), (cy, 0));
                    self.set_cursor((cy - 1, pl));
                } else {
                    self.b().delete(s, (cy, cx));
                    self.set_cursor(s);
                }
            }
            Key::Ctrl('u') => {
                let f = fnb(&self.bb().lines[cy]);
                let s = if cx > f { f } else { 0 };
                self.b().delete((cy, s), (cy, cx));
                self.set_cursor((cy, s));
            }
            Key::Ctrl('n') | Key::Ctrl('p') | Key::Ctrl(' ') => {
                self.update_completion(true);
                if let Some(c) = &mut self.comp {
                    c.sel = Some(if k == Key::Ctrl('p') { c.items.len() - 1 } else { 0 });
                }
                return;
            }
            Key::Ctrl('r') => {
                self.ins_reg_pending = true;
                return;
            }
            Key::Ctrl('s') => {
                self.ex("w");
                return;
            }
            Key::Ctrl('t') | Key::Ctrl('d') => {
                let before = self.bb().line_len(cy);
                self.indent_lines(cy, cy, k == Key::Ctrl('d'), 1);
                let after = self.bb().line_len(cy);
                self.set_cursor((cy, (cx + after).saturating_sub(before)));
            }
            Key::Left => {
                let x = prev_boundary(&self.bb().lines[cy], cx);
                self.set_cursor((cy, x));
            }
            Key::Right => {
                let x = next_boundary(&self.bb().lines[cy], cx);
                self.set_cursor((cy, x));
            }
            Key::Up | Key::Down => {
                let y = if k == Key::Up { cy.saturating_sub(1) } else { (cy + 1).min(self.bb().lines.len() - 1) };
                let want = self.bb().want_x;
                let ts = self.opts.tabstop;
                let b = self.b();
                b.cy = y;
                b.cx = byte_at_disp(&b.lines[y], want, ts);
            }
            Key::Home => self.set_cursor((cy, 0)),
            Key::End => {
                let l = self.bb().line_len(cy);
                self.set_cursor((cy, l));
            }
            _ => {}
        }
        self.comp = None;
    }

    fn update_completion(&mut self, manual: bool) {
        let (cy, cx) = self.cursor();
        if !manual {
            let l = &self.bb().lines[cy];
            let ws = complete::word_start(l, cx, false);
            let ps = complete::word_start(l, cx, true);
            if cx - ws < 2 && !l[ps..cx].contains('/') {
                self.comp = None;
                return;
            }
        }
        if self.sym_cache.is_none() {
            self.sym_cache = Some(complete::symbol_table(&self.bufs));
        }
        let old_sel = self.comp.as_ref().and_then(|c| c.sel.map(|s| c.items[s].text.clone()));
        self.comp = complete::complete(&self.bufs[self.cur], cy, cx, manual, self.sym_cache.as_ref().unwrap());
        if let (Some(c), Some(t)) = (&mut self.comp, old_sel) {
            c.sel = c.items.iter().position(|i| i.text == t);
        }
    }

    fn accept_completion(&mut self) {
        let Some(c) = self.comp.take() else { return };
        let Some(s) = c.sel else { return };
        let (cy, cx) = self.cursor();
        if cy != c.line || c.start > cx {
            return;
        }
        let text = c.items[s].text.clone();
        self.b().delete((cy, c.start), (cy, cx));
        let end = self.b().insert((cy, c.start), &text);
        self.set_cursor(end);
        if text.ends_with('/') {
            self.update_completion(false);
        }
    }

    // ---------- buffers & files ----------
    pub fn switch_buf(&mut self, i: usize) {
        if i < self.bufs.len() && i != self.cur {
            self.alt = self.cur;
            self.cur = i;
            self.comp = None;
            self.screen.invalidate();
        }
    }

    pub fn open_file(&mut self, path: &Path) -> usize {
        let canon = std::fs::canonicalize(path).ok();
        if let Some(c) = &canon {
            if let Some(i) = self.bufs.iter().position(|b| b.canonical().as_ref() == Some(c)) {
                self.switch_buf(i);
                return i;
            }
        }
        let nb = Buffer::new(Some(path.to_path_buf()));
        let exists = path.exists();
        let b = &self.bufs[self.cur];
        if b.path.is_none() && !b.dirty && b.lines.len() == 1 && b.lines[0].is_empty() {
            self.bufs[self.cur] = nb;
            self.screen.invalidate();
        } else {
            self.bufs.push(nb);
            let i = self.bufs.len() - 1;
            self.switch_buf(i);
        }
        let name = self.bb().name.clone();
        let n = self.bb().lines.len();
        if exists {
            self.info(format!("\"{}\" {}L", name, n));
        } else {
            self.info(format!("\"{}\" [new]", name));
        }
        self.cur
    }

    pub fn close_buf(&mut self, i: usize) {
        self.bufs.remove(i);
        if self.bufs.is_empty() {
            self.bufs.push(Buffer::new(None));
            self.cur = 0;
            self.alt = 0;
            self.screen.invalidate();
            return;
        }
        if self.cur >= i && self.cur > 0 {
            self.cur -= 1;
        }
        if self.alt >= self.bufs.len() || self.alt == i {
            self.alt = self.cur;
        } else if self.alt > i {
            self.alt -= 1;
        }
        self.screen.invalidate();
    }

    fn goto_file(&mut self) {
        let (cy, cx) = self.cursor();
        let l = &self.bb().lines[cy];
        let ok = |c: char| is_word(c) || "/.-~+".contains(c);
        let mut a = cx;
        while a > 0 && ok(l[prev_boundary(l, a)..].chars().next().unwrap()) {
            a = prev_boundary(l, a);
        }
        let mut e = cx;
        while e < l.len() && ok(l[e..].chars().next().unwrap()) {
            e = next_boundary(l, e);
        }
        let mut name = l[a..e].to_string();
        if let Some(r) = name.strip_prefix("~/") {
            name = format!("{}/{}", std::env::var("HOME").unwrap_or_default(), r);
        }
        if name.is_empty() {
            return;
        }
        let mut cands = vec![PathBuf::from(&name)];
        if let Some(dir) = self.bb().path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            cands.push(dir.join(&name));
        }
        match cands.into_iter().find(|p| p.is_file()) {
            Some(p) => {
                self.last_jump = (cy, cx);
                self.open_file(&p);
            }
            None => self.err(format!("no file: {}", name)),
        }
    }

    fn goto_definition(&mut self) {
        let Some(w) = self.word_under_cursor() else { return };
        let here = self.cursor();
        // current buffer first, then other open buffers
        let order: Vec<usize> = std::iter::once(self.cur).chain((0..self.bufs.len()).filter(|&i| i != self.cur)).collect();
        for bi in order {
            let b = &self.bufs[bi];
            let syms = complete::symbols(b.lang, &b.lines);
            if let Some((_, ln, _)) = syms.iter().find(|s| s.0 == w && (bi != self.cur || s.1 != here.0)) {
                let col = b.lines[*ln].find(w.as_str()).unwrap_or(0);
                let ln = *ln;
                self.last_jump = here;
                self.switch_buf(bi);
                self.set_cursor((ln, col));
                return;
            }
        }
        // search the project
        let lang = self.bb().lang;
        if lang.defs.is_empty() {
            self.err(format!("no definition found: {}", w));
            return;
        }
        let defs: Vec<String> = lang.defs.iter().map(|d| regex_escape(d)).collect();
        let pat = format!("\\b(?:{})\\s+{}\\b", defs.join("|"), regex_escape(&w));
        let Ok(re) = Regex::new(&pat, false) else { return };
        let root = std::env::current_dir().unwrap_or_default();
        let exts = lang.exts;
        let items: Vec<PickItem> = crate::picker::grep(&root, &re, 200).into_iter().filter(|it| it.path.as_ref().and_then(|p| p.extension()).map_or(false, |e| exts.contains(&e.to_string_lossy().as_ref()))).collect();
        match items.len() {
            0 => self.err(format!("no definition found: {}", w)),
            1 => {
                self.last_jump = here;
                self.open_pick(items[0].clone());
            }
            _ => self.picker = Some(Picker::new(&format!("definitions of {}", w), items)),
        }
    }

    // ---------- pickers ----------
    pub fn open_files_picker(&mut self) {
        let root = std::env::current_dir().unwrap_or_default();
        let files = crate::picker::walk_files(&root, 50000);
        let items = files.into_iter().map(|p| PickItem { label: p.display().to_string(), path: Some(p), buf: None, line: usize::MAX, col: 0 }).collect();
        self.picker = Some(Picker::new("files", items));
    }

    pub fn open_buffer_picker(&mut self) {
        let items = self.bufs.iter().enumerate().map(|(i, b)| PickItem { label: format!("{}{}", b.name, if b.dirty { " [+]" } else { "" }), path: None, buf: Some(i), line: usize::MAX, col: 0 }).collect();
        self.picker = Some(Picker::new("buffers", items));
    }

    pub fn open_diag_picker(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let items: Vec<PickItem> = self
            .qf
            .iter()
            .map(|(p, d)| {
                let rel = p.strip_prefix(&cwd).unwrap_or(p);
                let s = match d.sev {
                    Sev::Error => "E",
                    Sev::Warning => "W",
                    Sev::Info => "I",
                };
                PickItem { label: format!("{} {}:{}: {}", s, rel.display(), d.line + 1, d.msg), path: Some(p.clone()), buf: None, line: d.line, col: d.col }
            })
            .collect();
        if items.is_empty() {
            self.info("no diagnostics");
            return;
        }
        self.picker = Some(Picker::new("diagnostics", items));
    }

    pub fn open_symbol_picker(&mut self) {
        let b = self.bb();
        let syms = complete::symbols(b.lang, &b.lines);
        let items: Vec<PickItem> = syms.into_iter().map(|(_, ln, d)| PickItem { label: format!("{:>5}: {}", ln + 1, d), path: None, buf: Some(self.cur), line: ln, col: 0 }).collect();
        if items.is_empty() {
            self.info("no symbols");
            return;
        }
        self.picker = Some(Picker::new("symbols", items));
    }

    pub fn open_pick(&mut self, it: PickItem) {
        if let Some(bi) = it.buf {
            self.switch_buf(bi);
        } else if let Some(p) = &it.path {
            self.open_file(p);
        }
        if it.line != usize::MAX {
            let y = it.line.min(self.bb().lines.len() - 1);
            let l = &self.bb().lines[y];
            let x = l.char_indices().nth(it.col).map_or(it.col.min(l.len()), |(i, _)| i);
            self.set_cursor((y, x));
            let th = self.text_h();
            let b = self.b();
            if b.cy < b.top || b.cy >= b.top + th {
                b.top = b.cy.saturating_sub(th / 2);
            }
        }
    }

    fn picker_key(&mut self, k: Key) {
        let p = self.picker.as_mut().unwrap();
        match p.key(&k) {
            Pick::Continue => {}
            Pick::Cancel => self.picker = None,
            Pick::Accept => {
                let it = p.selected().cloned();
                self.picker = None;
                if let Some(it) = it {
                    let c = self.cursor();
                    self.last_jump = c;
                    self.open_pick(it);
                }
            }
        }
    }

    // ---------- diagnostics ----------
    pub fn run_check(&mut self, verbose: bool) {
        let b = self.bb();
        let Some(path) = b.path.clone() else {
            if verbose {
                self.err("no file");
            }
            return;
        };
        let checks: Vec<String> = match self.opts.checks.get(b.lang.name) {
            Some(v) => v.clone(),
            None => b.lang.check.iter().map(|s| s.to_string()).collect(),
        };
        if checks.is_empty() {
            if verbose {
                self.err(format!("no checker for {}", b.lang.name));
            }
            return;
        }
        if b.dirty {
            self.ex("w");
        }
        match crate::diag::pick(&checks, &path) {
            Some((cmd, cwd)) => {
                self.check_id += 1;
                self.check_running = true;
                self.check_cwd = cwd.clone();
                crate::diag::spawn(self.check_id, cmd, cwd, path, self.check_tx.clone());
            }
            None => {
                if verbose {
                    self.err(format!("no checker available for {} (tried: {})", self.bb().lang.name, checks.join(", ")));
                }
            }
        }
    }

    pub fn poll_check(&mut self) -> bool {
        let mut got = false;
        while let Ok(r) = self.check_rx.try_recv() {
            if r.id != self.check_id {
                continue;
            }
            got = true;
            self.check_running = false;
            let cwd = self.check_cwd.clone();
            for b in &mut self.bufs {
                let Some(cp) = b.canonical() else { continue };
                let in_scope = cp.starts_with(&cwd);
                let mine: Vec<Diag> = r.diags.iter().filter(|(p, _)| *p == cp).map(|(_, d)| d.clone()).collect();
                if in_scope || !mine.is_empty() {
                    b.diags = mine;
                    for d in &mut b.diags {
                        d.line = d.line.min(b.lines.len() - 1);
                    }
                }
            }
            let ne = r.diags.iter().filter(|d| d.1.sev == Sev::Error).count();
            let nw = r.diags.iter().filter(|d| d.1.sev == Sev::Warning).count();
            self.qf = r.diags;
            self.qf_idx = 0;
            if ne + nw > 0 {
                self.err(format!("check: {} error(s), {} warning(s) — ]d next, <space>d list", ne, nw));
            } else if !r.ok && self.qf.is_empty() {
                let first = r.raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").to_string();
                self.err(format!("check failed: {}", first));
                self.msg_lines = std::iter::once(format!("$ {}", r.cmd)).chain(r.raw.lines().take(200).map(|s| s.to_string())).collect();
            } else if self.qf.is_empty() {
                self.info("check: ok ✓");
            } else {
                self.info(format!("check: {} note(s)", self.qf.len()));
            }
        }
        got
    }

    fn jump_diag(&mut self, fwd: bool, errors_only: bool) {
        let (cy, cx) = self.cursor();
        let b = self.bb();
        let mut ds: Vec<(usize, usize)> = b.diags.iter().filter(|d| !errors_only || d.sev == Sev::Error).map(|d| {
            let l = &b.lines[d.line.min(b.lines.len() - 1)];
            (d.line, l.char_indices().nth(d.col).map_or(l.len(), |(i, _)| i))
        }).collect();
        ds.sort();
        ds.dedup();
        if ds.is_empty() {
            self.info("no diagnostics");
            return;
        }
        let t = if fwd { ds.iter().find(|&&p| p > (cy, cx)).or(ds.first()) } else { ds.iter().rev().find(|&&p| p < (cy, cx)).or(ds.last()) };
        if let Some(&p) = t {
            self.set_cursor(p);
            self.clamp();
        }
    }

    fn jump_qf(&mut self, fwd: bool) {
        if self.qf.is_empty() {
            self.info("no diagnostics");
            return;
        }
        let n = self.qf.len();
        self.qf_idx = if fwd { (self.qf_idx + 1) % n } else { (self.qf_idx + n - 1) % n };
        let (p, d) = self.qf[self.qf_idx].clone();
        self.open_pick(PickItem { label: String::new(), path: Some(p), buf: None, line: d.line, col: d.col });
        self.info(format!("({}/{}) {}", self.qf_idx + 1, n, d.msg));
    }

    fn show_line_diags(&mut self) {
        let cy = self.cursor().0;
        let lines: Vec<String> = self.bb().diags.iter().filter(|d| d.line == cy).map(|d| format!("{:?}: {}", d.sev, d.msg)).collect();
        if lines.is_empty() {
            self.info("no diagnostics on this line");
        } else {
            self.msg_lines = lines;
        }
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    fn run(text: &str, tw: usize, keys: &str) -> (String, Pos) {
        let mut e = Editor::new(80, 24);
        e.opts.textwidth = tw;
        let b = e.b();
        b.lines = text.split('\n').map(String::from).collect();
        for c in keys.chars() {
            e.handle_key(Key::Char(c));
        }
        (e.bb().lines.join("\n"), e.cursor())
    }

    #[test]
    fn gwip_wraps_paragraph_and_keeps_cursor() {
        let (t, c) = run("aaa bbb ccc ddd eee\nfff\n\nnext para", 10, "gwip");
        assert_eq!(t, "aaa bbb\nccc ddd\neee fff\n\nnext para");
        assert_eq!(c, (0, 0));
    }

    #[test]
    fn gqq_joins_and_moves_cursor() {
        let (t, c) = run("a\nb\nc", 20, "gqj");
        assert_eq!(t, "a b\nc");
        assert_eq!(c, (0, 0));
        let (_, c) = run("aaa bbb ccc ddd eee", 10, "gqq");
        assert_eq!(c.0, 2);
    }

    #[test]
    fn comment_leader_is_kept() {
        let (t, _) = run("    // one two three four five six", 20, "gww");
        assert_eq!(t, "    // one two three\n    // four five six");
    }
}

#[cfg(test)]
mod dot_tests {
    use super::*;

    fn run(text: &str, keys: &str) -> String {
        let mut e = Editor::new(80, 24);
        e.b().lines = text.split('\n').map(String::from).collect();
        for c in keys.chars() {
            e.handle_key(Key::Char(c));
        }
        e.bb().lines.join("\n")
    }

    #[test]
    fn dot_repeats_a_visual_indent_on_as_many_lines() {
        let out = run("a\nb\nc\nd\ne", "Vj>3j.");
        assert_eq!(out, "    a\n    b\nc\n    d\n    e");
    }

    #[test]
    fn dot_repeats_a_charwise_visual_delete() {
        assert_eq!(run("abcdef", "vld."), "ef");
    }
}
