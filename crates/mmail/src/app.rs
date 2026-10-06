// mmail state and sync. The server is the source of truth: every action is
// sent as a JMAP change and applied locally at once; the server's push
// notifications (including those caused by the web client or your phone)
// trigger a resync, which replaces whatever we guessed.

mod draw;
mod keys;

use crate::compose::{self, Draft};
use crate::jmap::{self, Net, Reply, Resp, Tag};
use crate::model::{self, Att, Identity, Mailbox, Mini, Msg, Row};
use crate::search;
use mtui::lineedit::LineEdit;
use mtui::picker::{Label, Picker};
use mtui::screen::Screen;
use mtui::sidebar;
use mtui::term;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const PAGE: usize = 60;
const MAX_ROWS: usize = 600;
/// Resync no more often than this, however many notifications arrive.
const SYNC_GAP: Duration = Duration::from_millis(400);
/// Resync this often when push isn't connected.
const POLL: Duration = Duration::from_secs(45);

pub const HELP: &str = "\
mailbox list:
  j k ↑ ↓      select              gg G  first / last   C-d C-u  page
  Enter l      open                x  mark (acts on all marked)   Esc  clear
  Tab h        folders <-> list    b  hide the folder pane
  J K (L H)    next / previous folder
  C-k          jump to a folder    /  search           C-r  resync
  c  compose   r  reply            R  reply all        f  forward
  e  archive   #  trash            !  spam
  u  read/unread                   s  star
  m  move to folder                M  add / remove a label (folder)
  q  quit      C-z suspend
search:  words, from: to: cc: subject: in:folder is:unread|read|starred|draft
         has:attachment before:2026-01-31 after:2026-01-01   (Esc leaves)
conversation:
  j k C-d C-u  scroll              g G  top / bottom   Space  page
  n p          next / prev message Enter  expand / collapse   a  all
  J K (L H)    next / prev conversation; the folder's conversations are the
               sidebar while you read (Tab moves there, j k switch)
  I            load the message's remote images (off by default; attached
               and embedded images show inline in kitty, ghostty, WezTerm)
  r R f        reply, reply all, forward (the message under the marker)
  e # ! u s m M  as in the list     E  edit a draft
  o            open a link         w  save an attachment
  q Esc h      back
writing: your editor ($VISUAL / $EDITOR) opens on the headers and body.
  Attach: /path/file adds a file; Keep: lines are existing attachments.
  After saving, y sends, d saves a draft, e edits again, q discards.
mouse:  click a folder, a row (double-click opens), an image (fullscreen),
        a message header (expands),
        an attachment (saves) or the links line; wheel scrolls; in the list the
        left columns are buttons: ✓ mark, ● read/unread, ★ star; right-click marks";

#[derive(Clone, PartialEq)]
pub enum View {
    Box(String),
    Search(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Side,
    List,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Op {
    Archive,
    Trash,
    Spam,
    Read,
    Star,
    Delete,
}

pub struct MbPick {
    pub id: String,
    pub label: String,
}

impl Label for MbPick {
    fn label(&self) -> &str {
        &self.label
    }
}

pub struct AttPick {
    pub att: Att,
    pub label: String,
}

impl Label for AttPick {
    fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum PickKind {
    Jump,
    Move,
    Label,
}

pub enum Mode {
    Normal,
    Search(LineEdit),
    /// Delete these messages for good (y/n).
    Confirm(Vec<Row>),
    Pick(Picker<MbPick>, PickKind, Vec<Row>),
    Links(Picker<String>),
    Atts(Picker<AttPick>),
    /// Edited; y sends, d saves a draft, e edits again, q discards.
    Send,
    Help,
    /// An image shown fullscreen, by gallery key.
    Image(String),
}

#[derive(Clone, PartialEq)]
pub enum Then {
    EditDraft(String),
    Reply,
    ReplyAll,
    Forward,
}

pub struct ThreadView {
    pub gen: u64,
    pub thread: String,
    pub subject: String,
    pub msgs: Vec<Msg>,
    pub cur: usize,
    pub scroll: usize,
    pub loaded: bool,
    /// Where each message starts in the drawn document (set by draw).
    pub starts: Vec<usize>,
    /// What to do once the messages have loaded.
    pub then: Option<Then>,
    /// Jump to the marker after the next draw.
    pub follow: bool,
}

pub struct Compose {
    pub draft: Draft,
    pub text: String,
    pub line: usize,
}

struct Outbox {
    send: bool,
    files: Vec<Option<Att>>,
}

/// What a row of a drawn conversation does when clicked.
#[derive(Clone, Copy, PartialEq)]
pub enum Hit {
    None,
    Head,
    Att(usize),
    Links,
}

#[derive(Default)]
pub struct Geo {
    pub side: sidebar::Geo,
    /// The sidebar row index to what it stands for (a folder, or a list row).
    pub side_map: Vec<Option<usize>>,
    pub lx0: usize,
    pub y0: usize,
    pub rows: usize,
    pub top: usize,
    /// For each visible row of an open conversation: its message and click action.
    pub trows: Vec<(usize, Hit)>,
}

pub struct App {
    pub screen: Screen,
    pub(crate) net: Net,
    pub quit: bool,
    pub suspend: bool,
    pub(crate) me: String,
    pub(crate) boxes: Vec<Mailbox>,
    pub(crate) ids: Vec<Identity>,
    pub(crate) view: Option<View>,
    prev_view: Option<View>,
    view_gen: u64,
    seq: u64,
    seen_seq: u64,
    pub(crate) rows: Vec<Row>,
    pub(crate) total: usize,
    pub(crate) loaded: bool,
    more_busy: bool,
    pub(crate) sel: usize,
    pub(crate) marked: HashSet<String>,
    pub(crate) focus: Focus,
    pub(crate) side_sel: usize,
    pub(crate) show_side: bool,
    pub(crate) side_st: sidebar::State,
    pub(crate) tside_st: sidebar::State,
    pub(crate) gallery: mimg::Gallery,
    /// Messages whose remote images the user chose to load.
    pub(crate) remote_ok: HashSet<String>,
    pub(crate) thread: Option<ThreadView>,
    thread_gen: u64,
    pub(crate) mode: Mode,
    pub(crate) msg: String,
    pub(crate) err: bool,
    pub(crate) pending: usize,
    pub(crate) live: bool,
    dirty: bool,
    last_sync: Instant,
    pub(crate) geo: Geo,
    pub(crate) last_click: Option<(Instant, usize)>,
    pub(crate) pending_g: bool,
    pub(crate) compose: Option<Compose>,
    outbox: Option<Outbox>,
    want_editor: bool,
    downloads: String,
}

fn downloads_dir() -> String {
    std::env::var("MMAIL_DOWNLOADS").ok().filter(|s| !s.is_empty()).or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/Downloads", h))).unwrap_or_else(|| ".".into())
}

impl App {
    pub fn new(w: usize, h: usize, net: Net, me: String, images: bool) -> App {
        let mut app = App {
            screen: Screen::new(w, h),
            net,
            quit: false,
            suspend: false,
            me,
            boxes: Vec::new(),
            ids: Vec::new(),
            view: None,
            prev_view: None,
            view_gen: 0,
            seq: 0,
            seen_seq: 0,
            rows: Vec::new(),
            total: 0,
            loaded: false,
            more_busy: false,
            sel: 0,
            marked: HashSet::new(),
            focus: Focus::List,
            side_sel: 0,
            show_side: w >= 80,
            side_st: Default::default(),
            tside_st: Default::default(),
            gallery: mimg::Gallery::new(images),
            remote_ok: HashSet::new(),
            thread: None,
            thread_gen: 0,
            mode: Mode::Normal,
            msg: "connecting…".into(),
            err: false,
            pending: 0,
            live: false,
            dirty: false,
            last_sync: Instant::now(),
            geo: Geo::default(),
            last_click: None,
            pending_g: false,
            compose: None,
            outbox: None,
            want_editor: false,
            downloads: downloads_dir(),
        };
        let account = app.acct();
        app.call(Tag::Mailboxes, jmap::mailboxes(&account));
        app.call(Tag::Identities, jmap::identities(&account));
        app.net.start_push();
        app
    }

    pub(crate) fn info(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.err = false;
    }

    pub(crate) fn error(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.err = true;
    }

    fn acct(&self) -> String {
        self.net.account().to_string()
    }

    fn call(&mut self, tag: Tag, calls: Vec<String>) {
        self.pending += 1;
        self.net.call(tag, calls);
    }

    // ---- mailboxes and views ----

    pub(crate) fn role(&self, role: &str) -> Option<&Mailbox> {
        self.boxes.iter().find(|b| b.role.as_deref() == Some(role))
    }

    pub(crate) fn mailbox(&self, id: &str) -> Option<&Mailbox> {
        self.boxes.iter().find(|b| b.id == id)
    }

    pub(crate) fn box_path(&self, id: &str) -> String {
        let mut parts = Vec::new();
        let mut cur = self.mailbox(id);
        while let Some(b) = cur {
            parts.push(b.name.clone());
            cur = b.parent.as_deref().and_then(|p| self.mailbox(p));
            if parts.len() > 8 {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    }

    /// The mailbox whose rows are shown (None while searching).
    pub(crate) fn cur_box(&self) -> Option<&str> {
        match &self.view {
            Some(View::Box(b)) => Some(b),
            _ => None,
        }
    }

    pub(crate) fn set_view(&mut self, v: View) {
        if matches!(v, View::Search(_)) {
            if !matches!(self.view, Some(View::Search(_))) {
                self.prev_view = self.view.clone();
            }
        } else {
            self.prev_view = None;
        }
        if let View::Box(b) = &v {
            if let Some(i) = self.boxes.iter().position(|m| m.id == *b) {
                self.side_sel = i;
            }
        }
        self.view = Some(v);
        self.view_gen += 1;
        self.rows.clear();
        self.marked.clear();
        self.total = 0;
        self.sel = 0;
        self.loaded = false;
        self.more_busy = false;
        self.thread = None;
        self.geo.top = 0;
        self.fetch_list(false);
    }

    pub(crate) fn leave_search(&mut self) {
        let v = self.prev_view.take().or_else(|| self.role("inbox").map(|b| View::Box(b.id.clone())));
        if let Some(v) = v {
            self.set_view(v);
        }
    }

    fn filter_json(&self) -> Option<String> {
        match self.view.as_ref()? {
            View::Box(b) => Some(format!("{{\"inMailbox\":{}}}", mtui::json::quote(b))),
            View::Search(q) => search::filter(q, &self.boxes).ok(),
        }
    }

    /// Fetch the first page (replacing the list, as many rows as are loaded)
    /// or, with `more`, the page after what is loaded.
    fn fetch_list(&mut self, more: bool) {
        let Some(f) = self.filter_json() else { return };
        let drafts = self.cur_box().and_then(|b| self.mailbox(b)).is_some_and(|b| b.role.as_deref() == Some("drafts"));
        let (pos, limit) = if more { (self.rows.len(), PAGE) } else { (0, self.rows.len().clamp(PAGE, MAX_ROWS)) };
        self.seq += 1;
        let tag = Tag::List(self.view_gen, self.seq, more);
        self.call(tag, jmap::list(&self.acct(), &f, !drafts, pos, limit));
    }

    pub(crate) fn load_more(&mut self) {
        if !self.more_busy && self.loaded && self.rows.len() < self.total && self.rows.len() < MAX_ROWS {
            self.more_busy = true;
            self.fetch_list(true);
        }
    }

    pub(crate) fn run_search(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        match search::filter(q, &self.boxes) {
            Ok(_) => self.set_view(View::Search(q.to_string())),
            Err(e) => self.error(e),
        }
    }

    /// Everything the server may have changed under us: folders and counts,
    /// the list, and the open conversation.
    pub(crate) fn sync(&mut self) {
        self.last_sync = Instant::now();
        self.dirty = false;
        let a = self.acct();
        self.call(Tag::Mailboxes, jmap::mailboxes(&a));
        if self.view.is_some() {
            self.fetch_list(false);
        }
        if let Some(t) = &self.thread {
            let (gen, id) = (t.gen, t.thread.clone());
            self.call(Tag::Thread(gen), jmap::thread(&a, &id));
        }
    }

    pub fn tick(&mut self) -> i32 {
        let since = self.last_sync.elapsed();
        if (self.dirty && since >= SYNC_GAP) || (!self.live && self.view.is_some() && since >= POLL) {
            self.sync();
        }
        if self.dirty {
            SYNC_GAP.as_millis() as i32
        } else {
            1000
        }
    }

    // ---- replies ----

    pub fn on_reply(&mut self, (tag, res): Reply) {
        match tag {
            Tag::Push => {
                match res {
                    Ok(r) if r.calls.is_empty() => {
                        // (re)connected: anything may have changed while we were away
                        self.live = true;
                        self.dirty = self.view.is_some();
                        if self.msg.starts_with("push:") {
                            self.info("push connected");
                        }
                    }
                    Ok(_) => self.dirty = true,
                    Err(_) => {}
                }
                return;
            }
            Tag::PushDown(why) => {
                self.live = false;
                if !why.is_empty() && self.msg != "connecting…" {
                    self.error(format!("push: {} (polling)", why));
                }
                return;
            }
            _ => {}
        }
        self.pending = self.pending.saturating_sub(1);
        match tag {
            Tag::Mailboxes => match res.and_then(|r| r.get("m").cloned()) {
                Ok(v) => self.on_mailboxes(&v),
                Err(e) => self.error(format!("folders: {}", e)),
            },
            Tag::Identities => match res.and_then(|r| r.get("i").cloned()) {
                Ok(v) => self.ids = v.get("list").arr().iter().map(Identity::parse).collect(),
                Err(e) => self.error(format!("identities: {}", e)),
            },
            Tag::List(gen, seq, more) => self.on_list(gen, seq, more, res),
            Tag::Thread(gen) => self.on_thread(gen, res),
            Tag::Act(label) => self.on_act(label, res),
            Tag::Send(what) => self.on_send(what, res),
            Tag::Upload(i) => self.on_upload(i, res),
            Tag::Download(name) => self.on_download(name, res),
            Tag::Image(key) => self.gallery.arrived(&key, res.map(|r| r.bytes), &self.screen),
            Tag::Push | Tag::PushDown(_) => {}
        }
    }

    fn on_mailboxes(&mut self, v: &mtui::json::Value) {
        let all: Vec<Mailbox> = v.get("list").arr().iter().map(Mailbox::parse).collect();
        self.boxes = model::tree(all);
        if self.view.is_none() {
            if let Some(b) = self.role("inbox").or(self.boxes.first()) {
                let id = b.id.clone();
                self.info(self.me.clone());
                self.set_view(View::Box(id));
            }
        } else if let Some(View::Box(b)) = &self.view {
            if let Some(i) = self.boxes.iter().position(|m| m.id == *b) {
                self.side_sel = i;
            } else {
                // the folder was deleted elsewhere
                if let Some(inbox) = self.role("inbox").map(|b| b.id.clone()) {
                    self.set_view(View::Box(inbox));
                }
            }
        }
    }

    fn on_list(&mut self, gen: u64, seq: u64, more: bool, res: Result<Resp, String>) {
        if gen != self.view_gen {
            return;
        }
        if more {
            self.more_busy = false;
        } else if seq < self.seen_seq {
            return;
        }
        let r = match res {
            Ok(r) => r,
            Err(e) => return self.error(e),
        };
        let (q, e, all) = match (r.get("q"), r.get("e"), r.get("all")) {
            (Ok(q), Ok(e), Ok(a)) => (q, e, a),
            (Err(x), ..) | (_, Err(x), _) | (.., Err(x)) => return self.error(x),
        };
        if !more {
            self.seen_seq = seq;
        }
        let mut threads: HashMap<String, Vec<Mini>> = HashMap::new();
        for m in all.get("list").arr() {
            threads.entry(m.get("threadId").str().to_string()).or_default().push(Mini::parse(m));
        }
        for v in threads.values_mut() {
            v.sort_by(|a, b| a.date.cmp(&b.date));
        }
        let order: Vec<&str> = q.get("ids").arr().iter().map(|i| i.str()).collect();
        let mut got: Vec<&mtui::json::Value> = e.get("list").arr().iter().collect();
        got.sort_by_key(|m| order.iter().position(|i| *i == m.get("id").str()).unwrap_or(usize::MAX));
        let view = self.cur_box().map(String::from);
        let rows: Vec<Row> = got.into_iter().map(|m| Row::parse(m, &threads, view.as_deref())).collect();
        self.total = q.get("total").num() as usize;
        let keep = self.rows.get(self.sel).map(|r| r.thread.clone());
        if more {
            for r in rows {
                if !self.rows.iter().any(|x| x.thread == r.thread) {
                    self.rows.push(r);
                }
            }
        } else {
            self.rows = rows;
            if let Some(k) = keep {
                if let Some(p) = self.rows.iter().position(|r| r.thread == k) {
                    self.sel = p;
                }
            }
            let live: HashSet<&String> = self.rows.iter().map(|r| &r.thread).collect();
            self.marked.retain(|t| live.contains(t));
        }
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        self.loaded = true;
    }

    // ---- conversations ----

    pub(crate) fn open_row(&mut self, i: usize, then: Option<Then>) {
        let Some(r) = self.rows.get(i).cloned() else { return };
        self.sel = i;
        self.focus = Focus::List;
        let in_drafts = r.draft && self.role("drafts").is_some_and(|d| self.cur_box() == Some(d.id.as_str()));
        let then = then.or(in_drafts.then(|| Then::EditDraft(r.id.clone())));
        self.thread_gen += 1;
        self.thread = Some(ThreadView { gen: self.thread_gen, thread: r.thread.clone(), subject: r.subject.clone(), msgs: Vec::new(), cur: 0, scroll: 0, loaded: false, starts: Vec::new(), then, follow: true });
        let (gen, a) = (self.thread_gen, self.acct());
        self.call(Tag::Thread(gen), jmap::thread(&a, &r.thread));
    }

    fn on_thread(&mut self, gen: u64, res: Result<Resp, String>) {
        let Some(t) = &mut self.thread else { return };
        if t.gen != gen {
            return;
        }
        let r = match res.and_then(|r| r.get("e").cloned()) {
            Ok(v) => v,
            Err(e) => return self.error(format!("conversation: {}", e)),
        };
        let mut msgs: Vec<Msg> = r.get("list").arr().iter().map(Msg::parse).collect();
        msgs.sort_by(|a, b| a.mini.date.cmp(&b.mini.date));
        if msgs.is_empty() {
            self.close_thread();
            return;
        }
        let was: HashMap<String, bool> = t.msgs.iter().map(|m| (m.mini.id.clone(), m.expanded)).collect();
        let cur_id = t.msgs.get(t.cur).map(|m| m.mini.id.clone());
        let first_load = !t.loaded;
        let last = msgs.len() - 1;
        for (i, m) in msgs.iter_mut().enumerate() {
            m.expanded = was.get(&m.mini.id).copied().unwrap_or(m.mini.unread() || i == last);
        }
        t.cur = cur_id.and_then(|c| msgs.iter().position(|m| m.mini.id == c)).unwrap_or_else(|| msgs.iter().position(|m| m.mini.unread()).unwrap_or(last));
        if first_load {
            t.subject = msgs[0].subject.clone();
            t.follow = true;
        }
        t.loaded = true;
        let then = t.then.take();
        // Opening shows the messages, so they are read (as in the web client).
        let unseen: Vec<String> = msgs.iter().filter(|m| m.mini.unread()).map(|m| m.mini.id.clone()).collect();
        t.msgs = msgs;
        let cur = t.cur;
        let draft = then.and_then(|w| match w {
            Then::EditDraft(id) => t.msgs.iter().find(|m| m.mini.id == id).map(compose::from_draft),
            Then::Reply => t.msgs.get(last).map(|m| compose::reply(m, false, &self.ids)),
            Then::ReplyAll => t.msgs.get(last).map(|m| compose::reply(m, true, &self.ids)),
            Then::Forward => t.msgs.get(last).map(|m| compose::forward(m, &self.ids)),
        });
        let _ = cur;
        if let Some(d) = draft {
            return self.start_compose(d);
        }
        if !unseen.is_empty() {
            let patches: Vec<Patch> = unseen.into_iter().map(|id| Patch { id, kw: vec![("$seen".into(), true)], ..Patch::default() }).collect();
            self.send_patches("marked read", patches);
        }
    }

    pub(crate) fn close_thread(&mut self) {
        self.thread = None;
        self.focus = Focus::List;
    }

    /// The open conversation as a list row (its messages from the loaded bodies).
    pub(crate) fn thread_row(&self) -> Option<Row> {
        let t = self.thread.as_ref()?;
        if let Some(r) = self.rows.iter().find(|r| r.thread == t.thread) {
            return Some(r.clone());
        }
        let last = t.msgs.last()?;
        let mut r = Row { thread: t.thread.clone(), id: last.mini.id.clone(), subject: t.subject.clone(), date: last.mini.date.clone(), emails: t.msgs.iter().map(|m| Mini { from: m.from.clone(), ..m.mini.clone() }).collect(), ..Row::default() };
        r.recompute(self.cur_box());
        Some(r)
    }

    // ---- changes ----

    /// Rows an action applies to: the open conversation, the marked rows, or the selected one.
    pub(crate) fn targets(&self) -> Vec<Row> {
        if self.thread.is_some() {
            return self.thread_row().into_iter().collect();
        }
        if !self.marked.is_empty() {
            return self.rows.iter().filter(|r| self.marked.contains(&r.thread)).cloned().collect();
        }
        self.rows.get(self.sel).cloned().into_iter().collect()
    }

    fn scope<'a>(&self, r: &'a Row) -> Vec<&'a Mini> {
        match self.cur_box() {
            Some(b) => r.in_box(b),
            None => r.emails.iter().collect(),
        }
    }

    pub(crate) fn do_op(&mut self, op: Op, rows: Vec<Row>) {
        if rows.is_empty() {
            return;
        }
        let here = self.cur_box().map(String::from);
        let inbox = self.role("inbox").map(|b| b.id.clone());
        let (archive, trash, junk) = (self.role("archive").map(|b| b.id.clone()), self.role("trash").map(|b| b.id.clone()), self.role("junk").map(|b| b.id.clone()));
        let target = |id: &Option<String>, what: &str| id.clone().ok_or(format!("no {} folder", what));
        let mut patches: Vec<Patch> = Vec::new();
        let mut destroy: Vec<String> = Vec::new();
        let label = match op {
            Op::Archive => "archived",
            Op::Trash => "moved to trash",
            Op::Spam => "marked as spam",
            Op::Read => "updated",
            Op::Star => "updated",
            Op::Delete => "deleted",
        };
        for r in &rows {
            let scope = self.scope(r);
            match op {
                Op::Archive => {
                    let a = match target(&archive, "archive") {
                        Ok(a) => a,
                        Err(e) => return self.error(e),
                    };
                    let from = here.clone().or(inbox.clone());
                    for m in scope {
                        if let Some(f) = from.as_ref().filter(|f| **f != a && m.boxes.contains(f)) {
                            patches.push(Patch { id: m.id.clone(), rm: vec![f.clone()], add: vec![a.clone()], ..Patch::default() });
                        }
                    }
                }
                Op::Trash | Op::Spam => {
                    let t = match target(if op == Op::Trash { &trash } else { &junk }, if op == Op::Trash { "trash" } else { "spam" }) {
                        Ok(t) => t,
                        Err(e) => return self.error(e),
                    };
                    for m in scope {
                        if m.boxes != [t.clone()] {
                            patches.push(Patch { id: m.id.clone(), set: Some(t.clone()), ..Patch::default() });
                        }
                    }
                }
                Op::Delete => destroy.extend(scope.iter().map(|m| m.id.clone())),
                Op::Read => {
                    if r.unread {
                        for m in scope.iter().filter(|m| m.unread()) {
                            patches.push(Patch { id: m.id.clone(), kw: vec![("$seen".into(), true)], ..Patch::default() });
                        }
                    } else if let Some(m) = scope.iter().rev().find(|m| m.has("$seen")) {
                        patches.push(Patch { id: m.id.clone(), kw: vec![("$seen".into(), false)], ..Patch::default() });
                    }
                }
                Op::Star => {
                    if r.flagged {
                        for m in scope.iter().filter(|m| m.has("$flagged")) {
                            patches.push(Patch { id: m.id.clone(), kw: vec![("$flagged".into(), false)], ..Patch::default() });
                        }
                    } else if let Some(m) = scope.iter().next_back() {
                        patches.push(Patch { id: m.id.clone(), kw: vec![("$flagged".into(), true)], ..Patch::default() });
                    }
                }
            }
        }
        if !destroy.is_empty() {
            self.rows.retain(|r| !rows.iter().any(|x| x.thread == r.thread));
            self.after_removal();
            self.pending += 1;
            self.net.call(Tag::Act(label), jmap::destroy(&self.acct(), &destroy));
            return self.info(format!("{}…", label));
        }
        if patches.is_empty() {
            return self.info("nothing to change");
        }
        let n = rows.len();
        self.send_patches(label, patches);
        if !matches!(op, Op::Read | Op::Star) {
            self.info(if n > 1 { format!("{} conversations {}", n, label) } else { label.to_string() });
        }
        self.marked.clear();
    }

    pub(crate) fn move_to(&mut self, rows: Vec<Row>, to: &str, add_only: bool) {
        let here = self.cur_box().map(String::from);
        let mut patches = Vec::new();
        let mut toggle_off = false;
        if add_only {
            // toggle: remove when every message already has it
            toggle_off = rows.iter().all(|r| self.scope(r).iter().all(|m| m.boxes.iter().any(|b| b == to)));
        }
        for r in &rows {
            for m in self.scope(r) {
                let has = m.boxes.iter().any(|b| b == to);
                let p = if add_only {
                    if toggle_off && m.boxes.len() > 1 {
                        Patch { id: m.id.clone(), rm: vec![to.into()], ..Patch::default() }
                    } else if !toggle_off && !has {
                        Patch { id: m.id.clone(), add: vec![to.into()], ..Patch::default() }
                    } else {
                        continue;
                    }
                } else if has {
                    continue;
                } else if let Some(h) = &here {
                    Patch { id: m.id.clone(), rm: vec![h.clone()], add: vec![to.into()], ..Patch::default() }
                } else {
                    Patch { id: m.id.clone(), set: Some(to.into()), ..Patch::default() }
                };
                patches.push(p);
            }
        }
        if patches.is_empty() {
            return self.info("nothing to change");
        }
        let name = self.box_path(to);
        self.send_patches("moved", patches);
        self.info(if add_only { format!("{} {}", if toggle_off { "removed from" } else { "added to" }, name) } else { format!("moved to {}", name) });
        self.marked.clear();
    }

    /// Apply to what we hold, then tell the server.
    fn send_patches(&mut self, label: &'static str, patches: Vec<Patch>) {
        let view = self.cur_box().map(String::from);
        for p in &patches {
            for r in &mut self.rows {
                for m in r.emails.iter_mut().filter(|m| m.id == p.id) {
                    p.apply(m);
                }
            }
            if let Some(t) = &mut self.thread {
                for m in t.msgs.iter_mut().filter(|m| m.mini.id == p.id) {
                    p.apply(&mut m.mini);
                }
            }
        }
        for r in &mut self.rows {
            r.recompute(view.as_deref());
        }
        if let Some(b) = &view {
            self.rows.retain(|r| !r.in_box(b).is_empty());
        }
        self.after_removal();
        let wire: Vec<(String, String)> = patches.iter().map(|p| (p.id.clone(), p.json())).collect();
        self.pending += 1;
        self.net.call(Tag::Act(label), jmap::update(&self.acct(), &wire));
    }

    fn after_removal(&mut self) {
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        let live: HashSet<&String> = self.rows.iter().map(|r| &r.thread).collect();
        self.marked.retain(|t| live.contains(t));
        if let Some(t) = &self.thread {
            if self.cur_box().is_some() && !live.contains(&t.thread) && t.loaded {
                self.close_thread();
            }
        }
    }

    fn on_act(&mut self, label: &'static str, res: Result<Resp, String>) {
        let bad = match res.and_then(|r| r.get("u").cloned()) {
            Ok(v) => jmap::set_error(&v),
            Err(e) => Some(e),
        };
        if let Some(e) = &bad {
            self.error(format!("{}: {}", label, e));
        }
        // the server's answer is the truth, good or bad
        if bad_or_quiet(label, self.live, &bad) {
            self.dirty = true;
        }
    }

    // ---- writing ----

    pub(crate) fn start_compose(&mut self, d: Draft) {
        if self.ids.is_empty() {
            return self.error("no sending identity (does the token allow email submission?)");
        }
        let (text, line) = d.to_text();
        self.compose = Some(Compose { draft: d, text, line });
        self.want_editor = true;
    }

    pub fn wants_editor(&self) -> bool {
        self.want_editor
    }

    /// Hand the terminal to the editor, then take it back.
    pub fn run_editor(&mut self) {
        self.want_editor = false;
        let Some(c) = &self.compose else { return };
        let (text, line) = (c.text.clone(), c.line);
        term::disable_raw();
        let r = compose::edit(&text, line);
        let _ = term::enable_raw();
        self.screen.invalidate();
        let Some(c) = &mut self.compose else { return };
        match r {
            Ok(t) => {
                let d = c.draft.parse(&t);
                if d.is_blank() {
                    self.compose = None;
                    return self.info("nothing written; discarded");
                }
                // keep the facts about what we answer; take what was typed
                c.draft = d;
                c.text = t;
                self.mode = Mode::Send;
                self.msg = String::new();
            }
            Err(e) => {
                self.compose = None;
                self.error(e);
            }
        }
    }

    pub(crate) fn edit_again(&mut self) {
        if self.compose.is_some() {
            self.mode = Mode::Normal;
            self.want_editor = true;
        }
    }

    pub(crate) fn finish_compose(&mut self, send: bool) {
        let Some(c) = &self.compose else { return };
        if send {
            if let Err(e) = c.draft.check(&self.ids) {
                self.mode = Mode::Send;
                return self.error(e);
            }
        }
        if self.role("drafts").is_none() {
            self.mode = Mode::Send;
            return self.error("no Drafts folder");
        }
        let mut files = Vec::new();
        let mut paths = Vec::new();
        for p in &c.draft.attach {
            let path = compose::expand_home(p);
            match std::fs::read(&path) {
                Ok(data) => {
                    paths.push((path.clone(), data));
                    files.push(None);
                }
                Err(e) => {
                    self.mode = Mode::Send;
                    return self.error(format!("{}: {}", p, e));
                }
            }
        }
        self.mode = Mode::Normal;
        self.info(if send { "sending…" } else { "saving draft…" });
        let n = paths.len();
        self.outbox = Some(Outbox { send, files });
        for (i, (path, data)) in paths.into_iter().enumerate() {
            self.pending += 1;
            self.net.upload(i, compose::mime_for(&path), data);
        }
        if n == 0 {
            self.submit();
        }
    }

    fn on_upload(&mut self, i: usize, res: Result<Resp, String>) {
        let Some(o) = &mut self.outbox else { return };
        match res {
            Ok(r) => {
                let v = &r.calls[0].1;
                let name = self.compose.as_ref().and_then(|c| c.draft.attach.get(i)).map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_default();
                o.files[i] = Some(Att { blob: v.get("blobId").str().into(), name, typ: v.get("type").str().into(), size: v.get("size").num() as u64 });
                if o.files.iter().all(|f| f.is_some()) {
                    self.submit();
                }
            }
            Err(e) => {
                self.outbox = None;
                self.mode = Mode::Send;
                self.error(format!("upload: {}", e));
            }
        }
    }

    fn submit(&mut self) {
        let (Some(o), Some(c)) = (self.outbox.take(), &self.compose) else { return };
        let files: Vec<Att> = o.files.into_iter().flatten().collect();
        let a = self.acct();
        let drafts = self.role("drafts").map(|b| b.id.clone()).unwrap_or_default();
        let calls = if o.send {
            let Ok(id) = c.draft.check(&self.ids) else { return };
            let sent = self.role("sent").map(|b| b.id.clone());
            c.draft.send_calls(&a, &drafts, sent.as_deref(), &id.id, &files)
        } else {
            c.draft.save_calls(&a, &drafts, &files)
        };
        self.call(Tag::Send(if o.send { "sent" } else { "draft saved" }), calls);
    }

    fn on_send(&mut self, what: &'static str, res: Result<Resp, String>) {
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                self.mode = Mode::Send;
                return self.error(e);
            }
        };
        let mut bad = None;
        for id in ["d", "s", "u"] {
            match r.get(id) {
                Ok(v) => bad = bad.or_else(|| jmap::set_error(v)),
                Err(e) if r.calls.iter().any(|c| c.2 == id) => bad = bad.or(Some(e)),
                Err(_) => {}
            }
        }
        if let Some(e) = bad {
            // Keep what was written. If the draft got saved but not sent, a retry replaces it.
            if let (Ok(d), Some(c)) = (r.get("d"), &mut self.compose) {
                if let Some(id) = d.path("created.c.id").opt_str() {
                    c.draft.replaces = Some(id.to_string());
                }
            }
            self.mode = Mode::Send;
            return self.error(format!("not {}: {}", what, e));
        }
        self.compose = None;
        self.info(what);
        self.dirty = true;
    }

    // ---- links and attachments ----

    pub(crate) fn open_url(&mut self, u: &str) {
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let r = std::process::Command::new(cmd).arg(u).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        match r {
            Ok(_) => self.info(format!("opened {}", u)),
            Err(e) => self.error(format!("{}: {}", cmd, e)),
        }
    }

    pub(crate) fn save_att(&mut self, a: &Att) {
        self.info(format!("downloading {}…", a.name));
        self.pending += 1;
        self.net.download(&a.name, &a.blob, &a.name, &a.typ);
    }

    fn on_download(&mut self, name: String, res: Result<Resp, String>) {
        let r = match res {
            Ok(r) => r,
            Err(e) => return self.error(format!("{}: {}", name, e)),
        };
        let safe: String = name.chars().map(|c| if c == '/' || c.is_control() { '_' } else { c }).collect::<String>().trim_start_matches('.').to_string();
        let safe = if safe.is_empty() { "attachment".to_string() } else { safe };
        let _ = std::fs::create_dir_all(&self.downloads);
        let mut path = std::path::Path::new(&self.downloads).join(&safe);
        let mut n = 1;
        while path.exists() {
            let (stem, ext) = safe.rsplit_once('.').map_or((safe.as_str(), String::new()), |(s, e)| (s, format!(".{}", e)));
            path = std::path::Path::new(&self.downloads).join(format!("{} ({}){}", stem, n, ext));
            n += 1;
        }
        match std::fs::write(&path, &r.bytes) {
            Ok(()) => self.info(format!("saved {}", path.display())),
            Err(e) => self.error(format!("{}: {}", path.display(), e)),
        }
    }
}

/// Reading a conversation marks it read; the push that follows resyncs by itself.
fn bad_or_quiet(label: &str, live: bool, bad: &Option<String>) -> bool {
    bad.is_some() || !live || label != "marked read"
}

#[derive(Default, Clone)]
pub struct Patch {
    pub id: String,
    /// Replace the message's mailboxes with just this one.
    pub set: Option<String>,
    pub add: Vec<String>,
    pub rm: Vec<String>,
    pub kw: Vec<(String, bool)>,
}

impl Patch {
    pub fn apply(&self, m: &mut Mini) {
        if let Some(s) = &self.set {
            m.boxes = vec![s.clone()];
        }
        m.boxes.retain(|b| !self.rm.contains(b));
        for a in &self.add {
            if !m.boxes.contains(a) {
                m.boxes.push(a.clone());
            }
        }
        for (k, on) in &self.kw {
            m.keywords.retain(|x| x != k);
            if *on {
                m.keywords.push(k.clone());
            }
        }
    }

    /// The body of an `Email/set` update entry.
    pub fn json(&self) -> String {
        let q = mtui::json::quote;
        let mut p: Vec<String> = Vec::new();
        if let Some(s) = &self.set {
            p.push(format!("\"mailboxIds\":{{{}:true}}", q(s)));
        }
        for a in &self.add {
            p.push(format!("{}:true", q(&format!("mailboxIds/{}", a))));
        }
        for r in &self.rm {
            p.push(format!("{}:null", q(&format!("mailboxIds/{}", r))));
        }
        for (k, on) in &self.kw {
            p.push(format!("{}:{}", q(&format!("keywords/{}", k)), if *on { "true" } else { "null" }));
        }
        p.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches() {
        let mut m = Mini { id: "e".into(), boxes: vec!["inbox".into(), "label".into()], keywords: vec!["$flagged".into()], ..Mini::default() };
        let p = Patch { id: "e".into(), rm: vec!["inbox".into()], add: vec!["archive".into()], kw: vec![("$seen".into(), true), ("$flagged".into(), false)], ..Patch::default() };
        p.apply(&mut m);
        assert_eq!(m.boxes, vec!["label", "archive"]);
        assert_eq!(m.keywords, vec!["$seen"]);
        assert_eq!(p.json(), "\"mailboxIds/archive\":true,\"mailboxIds/inbox\":null,\"keywords/$seen\":true,\"keywords/$flagged\":null");
        let t = Patch { set: Some("trash".into()), ..Patch::default() };
        t.apply(&mut m);
        assert_eq!(m.boxes, vec!["trash"]);
        assert_eq!(t.json(), "\"mailboxIds\":{\"trash\":true}");
    }
}
