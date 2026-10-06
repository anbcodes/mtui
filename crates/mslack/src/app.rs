// mslack state, keys and drawing.

use crate::format::{self, Names, Rich, Span};
use crate::net::{Fetch, Net, Reply, Tag};
use mtui::fuzzy;
use mtui::json::Value;
use mtui::lineedit::{Edit, LineEdit};
use mtui::picker::{Label, Pick, Picker};
use mtui::screen::{Screen, Style, BOLD, ITALIC, UNDERLINE};
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::{str_width, wrap};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const FG_DIM: u8 = 242;
const BG_BAR: u8 = 236;
const BG_SIDE: u8 = 234;
const BG_SEL: u8 = 237;
const ACCENT: u8 = 180;
const C_LINK: u8 = 75;
const C_MENTION: u8 = 117;
const C_ME: u8 = 215;
const C_CODE: u8 = 180;
const C_ERR: u8 = 203;
const NAME_COLORS: [u8; 12] = [167, 173, 179, 143, 107, 72, 74, 110, 104, 140, 175, 139];

const POLL: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_secs(30);
const COUNTS: Duration = Duration::from_secs(30);
const PEEK: Duration = Duration::from_secs(3);
/// With Socket Mode connected, polling is only a slow safety net.
const LIVE_REFRESH: Duration = Duration::from_secs(300);
const LIVE_COUNTS: Duration = Duration::from_secs(600);
const CHANS_MIN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Public,
    Private,
    Mpim,
    Im,
}

pub struct Chan {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    user: Option<String>,
    msgs: Vec<Msg>,
    loaded: bool,
    more: bool,
    unread: bool,
    mentions: u32,
    latest: String,
    seeded: bool,
}

#[derive(Clone)]
struct Msg {
    ts: String,
    user: String,
    bot: Option<String>,
    text: String,
    sub: String,
    thread_ts: String,
    replies: u32,
    reactions: Vec<(String, u32, bool)>,
    files: Vec<String>,
    images: Vec<Pic>,
    edited: bool,
}

/// An image attachment we can show inline.
#[derive(Clone)]
struct Pic {
    name: String,
    url: String,
}

impl Msg {
    fn from(v: &Value, me: &str) -> Msg {
        let mut text = v.get("text").str().to_string();
        if text.is_empty() {
            if let Some(a) = v.get("attachments").arr().first() {
                text = a.get("fallback").opt_str().or(a.get("text").opt_str()).unwrap_or("").to_string();
            }
        }
        let bot = v.path("bot_profile.name").opt_str().or(v.get("username").opt_str()).map(String::from);
        Msg {
            ts: v.get("ts").str().into(),
            user: v.get("user").str().into(),
            bot,
            text,
            sub: v.get("subtype").str().into(),
            thread_ts: v.get("thread_ts").str().into(),
            replies: v.get("reply_count").num() as u32,
            reactions: v.get("reactions").arr().iter().map(|r| (r.get("name").str().to_string(), r.get("count").num() as u32, r.get("users").arr().iter().any(|u| u.str() == me))).collect(),
            files: v.get("files").arr().iter().map(|f| f.get("name").opt_str().or(f.get("title").opt_str()).unwrap_or("file").to_string()).collect(),
            images: v.get("files").arr().iter().filter(|f| matches!(f.get("mimetype").str(), "image/png" | "image/jpeg")).filter_map(|f| {
                let url = ["thumb_720", "thumb_480", "thumb_360", "url_private"].iter().find_map(|k| f.get(k).opt_str())?;
                Some(Pic { name: f.get("name").opt_str().or(f.get("title").opt_str()).unwrap_or("image").to_string(), url: url.to_string() })
            }).collect(),
            edited: !v.get("edited").is_null(),
        }
    }
}

struct Thread {
    chan: usize,
    ts: String,
    msgs: Vec<Msg>,
}

struct ChanItem(String, usize);
impl Label for ChanItem {
    fn label(&self) -> &str {
        &self.0
    }
}

enum Mode {
    Normal,
    Insert,
    Pick(Picker<ChanItem>),
    Links(Picker<String>),
    React(LineEdit),
    ConfirmDelete(String),
    Help,
}

/// One rendered row of the message pane.
enum Row {
    Day(String),
    Text { m: usize, head: bool, a: usize, b: usize },
    Extra { m: usize, s: String, kind: Extra },
    /// One line of an image (or its placeholder when it isn't shown yet).
    Img { m: usize, k: usize, i: usize, of: usize },
}

#[derive(Clone, Copy, PartialEq)]
enum Extra {
    Files,
    Reactions,
    Replies,
}

/// Something clickable drawn in the message pane.
enum Hit {
    Link(String),
    React(usize, String),
    Thread(usize),
}

/// Where the last frame put things, for mouse hit-testing.
#[derive(Default)]
struct Geo {
    side_w: usize,
    side_start: usize,
    y_comp: usize,
    comp_rows: usize,
    comp_x: usize,
    comp_w: usize,
    view_h: usize,
    /// message index under each screen row
    row_msg: Vec<Option<usize>>,
    hits: Vec<(usize, usize, usize, Hit)>,
}

pub struct App {
    pub screen: Screen,
    net: Net,
    pub quit: bool,
    pub suspend: bool,
    gallery: mimg::Gallery,
    me: String,
    team: String,
    chans: Vec<Chan>,
    cur: Option<usize>,
    want: Option<String>,
    thread: Option<Thread>,
    users: HashMap<String, String>,
    handles: HashMap<String, String>,
    chan_names: HashMap<String, String>,
    mode: Mode,
    input: LineEdit,
    editing: Option<String>,
    comp: Option<(usize, Vec<String>, usize)>,
    sel: Option<usize>,
    top: usize,
    rows_cache: Vec<usize>,
    pending_g: bool,
    sidebar: bool,
    msg: String,
    err: bool,
    inflight: HashSet<String>,
    last_poll: Instant,
    last_refresh: Instant,
    last_counts: Instant,
    last_peek: Instant,
    peek_idx: usize,
    counts_ok: Option<bool>,
    mark_ok: bool,
    socket: bool,
    live: bool,
    last_chans: Option<Instant>,
    /// Free scrolling (mouse wheel): the message at the top row and how many
    /// of its rows are scrolled past. None = keep the selection in view.
    pin: Option<(usize, usize)>,
    side_top: Option<usize>,
    geo: Geo,
    last_click: Option<(Instant, usize)>,
    /// Range selection (visual mode / mouse drag): the other end from `sel`.
    anchor: Option<usize>,
    drag_from: Option<usize>,
}

fn local_tm(ts: &str) -> libc::tm {
    let t = ts.split('.').next().unwrap_or("0").parse::<libc::time_t>().unwrap_or(0);
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

fn day_label(tm: &libc::tm) -> String {
    const D: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {} {}", D[tm.tm_wday as usize % 7], M[tm.tm_mon as usize % 12], tm.tm_mday, 1900 + tm.tm_year)
}

fn ts_f(ts: &str) -> f64 {
    ts.parse().unwrap_or(0.0)
}

fn name_color(s: &str) -> u8 {
    let h = s.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    NAME_COLORS[h as usize % NAME_COLORS.len()]
}

/// One reaction as shown under a message; `[…]` marks your own.
fn chip((n, c, me): &(String, u32, bool)) -> String {
    let e = format::emoji_char(n).map(String::from).unwrap_or(format!(":{}:", n));
    if *me {
        format!("[{} {}]", e, c)
    } else {
        format!(" {} {} ", e, c)
    }
}

fn urls(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        let Some(j) = rest[i..].find('>') else { break };
        let t = rest[i + 1..i + j].split('|').next().unwrap_or("");
        if t.starts_with("http") || t.starts_with("mailto:") {
            v.push(format::unescape(t));
        }
        rest = &rest[i + j + 1..];
    }
    v
}

impl App {
    pub fn new(w: usize, h: usize, net: Net, want: Option<String>, images: bool) -> App {
        let now = Instant::now();
        let app = App {
            screen: Screen::new(w, h),
            net,
            quit: false,
            suspend: false,
            gallery: mimg::Gallery::new(images),
            me: String::new(),
            team: String::new(),
            chans: Vec::new(),
            cur: None,
            want,
            thread: None,
            users: HashMap::new(),
            handles: HashMap::new(),
            chan_names: HashMap::new(),
            mode: Mode::Normal,
            input: LineEdit::new(),
            editing: None,
            comp: None,
            sel: None,
            top: 0,
            rows_cache: Vec::new(),
            pending_g: false,
            sidebar: true,
            msg: "connecting…".into(),
            err: false,
            inflight: HashSet::new(),
            last_poll: now,
            last_refresh: now,
            last_counts: now - COUNTS,
            last_peek: now,
            peek_idx: 0,
            counts_ok: None,
            mark_ok: true,
            socket: true,
            live: false,
            last_chans: None,
            pin: None,
            side_top: None,
            geo: Geo::default(),
            last_click: None,
            anchor: None,
            drag_from: None,
        };
        app.net.call(Tag::Auth, "auth.test", &[]);
        app
    }

    fn info(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.err = false;
    }

    fn error(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.err = true;
    }

    fn name_of(&self, m: &Msg) -> String {
        if let Some(n) = self.users.get(&m.user) {
            return n.clone();
        }
        m.bot.clone().unwrap_or_else(|| if m.user.is_empty() { "?".into() } else { m.user.clone() })
    }

    fn view_msgs(&self) -> &[Msg] {
        match (&self.thread, self.cur) {
            (Some(t), _) => &t.msgs,
            (None, Some(c)) => &self.chans[c].msgs,
            _ => &[],
        }
    }

    /// The selected messages (inclusive), if a range is active.
    fn range(&self) -> Option<(usize, usize)> {
        let (a, s) = (self.anchor?, self.sel?);
        Some((a.min(s), a.max(s)))
    }

    /// Messages a..=b as plain text: "[HH:MM] name: text", with a date line
    /// whenever the day changes.
    fn transcript(&self, a: usize, b: usize) -> String {
        let n = Names { users: &self.users, chans: &self.chan_names, me: &self.me };
        let msgs = self.view_msgs();
        let mut out = String::new();
        let mut day = -1;
        for m in &msgs[a..=b.min(msgs.len().saturating_sub(1))] {
            let tm = local_tm(&m.ts);
            let d = tm.tm_year * 400 + tm.tm_yday;
            if d != day {
                out += &format!("— {} —\n", day_label(&tm));
                day = d;
            }
            let text = format::render(&m.text, &n).text.replace('\n', "\n    ");
            out += &format!("[{:02}:{:02}] {}: {}\n", tm.tm_hour, tm.tm_min, self.name_of(m), text);
            for f in &m.files {
                out += &format!("    📎 {}\n", f);
            }
        }
        out
    }

    // ---------- networking ----------

    fn fetch(&mut self, key: String, tag: Tag, method: &str, params: &[(&str, &str)]) {
        if self.inflight.insert(key) {
            self.net.call(tag, method, params);
        }
    }

    fn load_history(&mut self, ci: usize, f: Fetch) {
        let c = &self.chans[ci];
        let id = c.id.clone();
        let (oldest, latest) = match f {
            Fetch::Newer => (c.msgs.last().map(|m| m.ts.clone()).unwrap_or_default(), String::new()),
            Fetch::Older => (String::new(), c.msgs.first().map(|m| m.ts.clone()).unwrap_or_default()),
            _ => (String::new(), String::new()),
        };
        let limit = if f == Fetch::Refresh { "30" } else { "50" };
        let mut p = vec![("channel", id.as_str()), ("limit", limit)];
        if !oldest.is_empty() {
            p.push(("oldest", &oldest));
        }
        if !latest.is_empty() {
            p.push(("latest", &latest));
        }
        self.fetch(format!("h:{}", id), Tag::History(id.clone(), f), "conversations.history", &p);
    }

    fn load_thread(&mut self) {
        let Some(t) = &self.thread else { return };
        let (id, ts) = (self.chans[t.chan].id.clone(), t.ts.clone());
        self.fetch(format!("r:{}", ts), Tag::Replies(ts.clone()), "conversations.replies", &[("channel", &id), ("ts", &ts), ("limit", "200")]);
    }

    fn mark_read(&mut self, ci: usize) {
        let c = &mut self.chans[ci];
        c.unread = false;
        c.mentions = 0;
        if let (true, Some(m)) = (self.mark_ok, c.msgs.last()) {
            let (id, ts) = (c.id.clone(), m.ts.clone());
            self.net.call(Tag::Mark, "conversations.mark", &[("channel", &id), ("ts", &ts)]);
        }
    }

    /// Periodic work. Returns how long the main loop may sleep.
    pub fn tick(&mut self) -> i32 {
        let now = Instant::now();
        if self.me.is_empty() {
            return 1000;
        }
        if let Some(ci) = self.cur {
            let every = if self.live { LIVE_REFRESH } else { POLL };
            if self.chans[ci].loaded && now - self.last_poll >= every {
                self.last_poll = now;
                if self.live || now - self.last_refresh >= REFRESH {
                    self.last_refresh = now;
                    self.load_history(ci, Fetch::Refresh);
                } else {
                    self.load_history(ci, Fetch::Newer);
                }
                self.load_thread();
            }
        }
        let counts_every = if self.live { LIVE_COUNTS } else { COUNTS };
        if self.counts_ok != Some(false) && now - self.last_counts >= counts_every {
            self.last_counts = now;
            self.fetch("counts".into(), Tag::Counts, "client.counts", &[]);
        } else if !self.live && self.counts_ok == Some(false) && !self.chans.is_empty() && now - self.last_peek >= PEEK {
            // No unread counts for this token type: round-robin a cheap
            // "latest message" check over the other conversations.
            self.last_peek = now;
            self.peek_idx = (self.peek_idx + 1) % self.chans.len();
            if Some(self.peek_idx) != self.cur {
                let id = self.chans[self.peek_idx].id.clone();
                self.fetch(format!("h:{}", id), Tag::Peek(id.clone()), "conversations.history", &[("channel", &id), ("limit", "1")]);
            }
        }
        1000
    }

    fn chan_idx(&self, id: &str) -> Option<usize> {
        self.chans.iter().position(|c| c.id == id)
    }

    fn want_user(&mut self, uid: &str) {
        if !uid.is_empty() && !self.users.contains_key(uid) && uid.starts_with(['U', 'W']) {
            let uid = uid.to_string();
            self.fetch(format!("u:{}", uid), Tag::User(uid.clone()), "users.info", &[("user", &uid)]);
        }
    }

    fn add_user(&mut self, u: &Value) {
        let p = u.get("profile");
        let name = p.get("display_name").opt_str().or(p.get("real_name").opt_str()).or(u.get("name").opt_str()).unwrap_or("?").to_string();
        let id = u.get("id").str().to_string();
        if let Some(h) = u.get("name").opt_str() {
            self.handles.insert(h.to_lowercase(), id.clone());
        }
        self.handles.entry(name.replace(' ', "").to_lowercase()).or_insert(id.clone());
        self.users.insert(id, name);
    }

    fn dm_names(&mut self) {
        for c in &mut self.chans {
            if let (Kind::Im, Some(u)) = (c.kind, &c.user) {
                c.name = self.users.get(u).cloned().unwrap_or_else(|| u.clone());
            }
        }
    }

    pub fn on_reply(&mut self, (tag, res): Reply) {
        if let Tag::Image(k) = &tag {
            let bytes = self.net.take_image(k);
            self.gallery.arrived(k, bytes, &self.screen);
            return;
        }
        let key = match &tag {
            Tag::History(id, _) | Tag::Peek(id) => Some(format!("h:{}", id)),
            Tag::Replies(ts) => Some(format!("r:{}", ts)),
            Tag::User(u) => Some(format!("u:{}", u)),
            Tag::Counts => Some("counts".into()),
            _ => None,
        };
        if let Some(k) = key {
            self.inflight.remove(&k);
        }
        match (&tag, &res) {
            (Tag::Live, Ok(_)) => {
                let reconnect = !self.live && self.last_chans.is_some();
                self.live = true;
                self.info("live updates connected");
                if reconnect {
                    // catch up on anything missed while disconnected
                    if let Some(ci) = self.cur {
                        self.load_history(ci, Fetch::Refresh);
                    }
                    self.load_thread();
                    self.last_counts = Instant::now() - LIVE_COUNTS;
                }
                return;
            }
            (Tag::Live, Err(e)) if e.starts_with("stopped: ") => {
                // Live events aren't available with these credentials.
                self.live = false;
                self.socket = false;
                if !e.contains("rtm.connect: not_allowed_token_type") && !e.contains("rtm.connect: method_deprecated") {
                    self.error(format!("live updates off ({}); polling", &e[9..]));
                }
                return;
            }
            (Tag::Live, Err(e)) => {
                if self.live {
                    self.error(format!("live link lost ({}); polling until it reconnects", e));
                }
                self.live = false;
                return;
            }
            (Tag::Event, Ok(ev)) => {
                if !self.me.is_empty() {
                    self.on_event(ev);
                }
                return;
            }
            _ => {}
        }
        let v = match res {
            Ok(v) => v,
            Err(e) => {
                match tag {
                    Tag::Counts => self.counts_ok = Some(false),
                    Tag::Mark => self.mark_ok = false,
                    Tag::Peek(_) | Tag::User(_) => {}
                    Tag::Auth if e.contains("auth") => self.error(format!("auth failed: {} (check your token)", e)),
                    Tag::Auth => self.error(format!("cannot reach Slack: {}", e)),
                    t => self.error(format!("{:?}: {}", t, e).replace("\"", "")),
                }
                return;
            }
        };
        match tag {
            Tag::Auth => {
                self.me = v.get("user_id").str().into();
                self.team = v.get("team").str().into();
                self.info(format!("connected to {} as {}", self.team, v.get("user").str()));
                self.refetch_chans();
                self.net.call_all(Tag::Users, "users.list", &[("limit", "500")], "members", 40);
            }
            Tag::Users => {
                for u in v.get("members").arr() {
                    if !u.get("deleted").bool() {
                        self.add_user(u);
                    }
                }
                self.dm_names();
            }
            Tag::User(_) => {
                self.add_user(v.get("user"));
                self.dm_names();
            }
            Tag::Chans => self.on_chans(&v),
            Tag::History(id, f) => self.on_history(&id, f, &v),
            Tag::Replies(ts) => {
                let me = self.me.clone();
                let msgs: Vec<Msg> = v.get("messages").arr().iter().map(|m| Msg::from(m, &me)).collect();
                for m in &msgs {
                    self.want_user(&m.user.clone());
                }
                if let Some(t) = &mut self.thread {
                    if t.ts == ts && !msgs.is_empty() {
                        t.msgs = msgs;
                    }
                }
            }
            Tag::Posted(id, thread) => {
                let me = self.me.clone();
                let m = Msg::from(v.get("message"), &me);
                if thread.is_some() {
                    self.load_thread();
                } else if let Some(ci) = self.chan_idx(&id) {
                    let c = &mut self.chans[ci];
                    if !c.msgs.iter().any(|x| x.ts == m.ts) {
                        c.latest = m.ts.clone();
                        c.msgs.push(m);
                    }
                    self.sel = None;
                }
            }
            Tag::Done(what) => {
                self.info(what);
                if let Some(ci) = self.cur {
                    self.load_history(ci, Fetch::Refresh);
                }
                self.load_thread();
            }
            Tag::Image(_) => {}
            Tag::Counts => {
                self.counts_ok = Some(true);
                for k in ["channels", "mpims", "ims"] {
                    for c in v.get(k).arr() {
                        if let Some(ci) = self.chan_idx(c.get("id").str()) {
                            if Some(ci) != self.cur {
                                let ch = &mut self.chans[ci];
                                ch.unread = c.get("has_unreads").bool();
                                ch.mentions = c.get("mention_count").num() as u32;
                            }
                        }
                    }
                }
            }
            Tag::Peek(id) => {
                let Some(ci) = self.chan_idx(&id) else { return };
                let me = self.me.clone();
                let Some(m) = v.get("messages").arr().first().map(|m| Msg::from(m, &me)) else { return };
                let c = &mut self.chans[ci];
                if c.seeded && ts_f(&m.ts) > ts_f(&c.latest) && m.user != me {
                    c.unread = true;
                    if c.kind == Kind::Im || m.text.contains(&format!("<@{}>", me)) {
                        c.mentions += 1;
                        self.screen.raw(b"\x07");
                    }
                }
                c.seeded = true;
                if ts_f(&m.ts) > ts_f(&c.latest) {
                    c.latest = m.ts;
                }
            }
            Tag::Mark | Tag::Live | Tag::Event => {}
        }
    }

    // ---------- pushed events ----------

    fn on_event(&mut self, ev: &Value) {
        let me = self.me.clone();
        match ev.get("type").str() {
            "message" => {
                let Some(ci) = self.chan_idx(ev.get("channel").str()) else {
                    self.refetch_chans();
                    return;
                };
                match ev.get("subtype").str() {
                    "message_changed" => {
                        let m = Msg::from(ev.get("message"), &me);
                        self.each_msg(ci, &m.ts.clone(), |x| *x = m.clone());
                    }
                    "message_deleted" => {
                        let ts = ev.get("deleted_ts").str().to_string();
                        self.remove_msg(ci, &ts);
                    }
                    "message_replied" => {}
                    _ => self.new_msg(ci, Msg::from(ev, &me)),
                }
            }
            t @ ("reaction_added" | "reaction_removed") => {
                let item = ev.get("item");
                let Some(ci) = self.chan_idx(item.get("channel").str()) else { return };
                let (name, user) = (ev.get("reaction").str().to_string(), ev.get("user").str().to_string());
                let mine = user == me;
                let add = t == "reaction_added";
                self.each_msg(ci, &item.get("ts").str().to_string(), |m| {
                    let i = m.reactions.iter().position(|r| r.0 == name);
                    match (i, add) {
                        // our own optimistic update already counted it
                        (Some(i), true) if !(mine && m.reactions[i].2) => {
                            m.reactions[i].1 += 1;
                            m.reactions[i].2 |= mine;
                        }
                        (None, true) => m.reactions.push((name.clone(), 1, mine)),
                        (Some(i), false) if !(mine && !m.reactions[i].2) => {
                            m.reactions[i].1 = m.reactions[i].1.saturating_sub(1);
                            if mine {
                                m.reactions[i].2 = false;
                            }
                            m.reactions.retain(|r| r.1 > 0);
                        }
                        _ => {}
                    }
                });
            }
            "channel_joined" | "group_joined" | "im_created" | "channel_rename" | "group_rename" | "channel_left" | "group_left" | "channel_archive" => self.refetch_chans(),
            "member_joined_channel" if ev.get("user").str() == me => self.refetch_chans(),
            // read on another device (RTM only)
            "channel_marked" | "group_marked" | "im_marked" | "mpim_marked" => {
                if let Some(ci) = self.chan_idx(ev.get("channel").str()) {
                    if Some(ci) != self.cur {
                        let c = &mut self.chans[ci];
                        c.unread = ev.get("unread_count_display").num() > 0.0;
                        if !c.unread {
                            c.mentions = 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Apply `f` to the message with `ts` in a channel and its open thread.
    fn each_msg(&mut self, ci: usize, ts: &str, mut f: impl FnMut(&mut Msg)) {
        if let Some(m) = self.chans[ci].msgs.iter_mut().find(|m| m.ts == ts) {
            f(m);
        }
        if let Some(t) = self.thread.as_mut().filter(|t| t.chan == ci) {
            if let Some(m) = t.msgs.iter_mut().find(|m| m.ts == ts) {
                f(m);
            }
        }
    }

    fn remove_msg(&mut self, ci: usize, ts: &str) {
        self.chans[ci].msgs.retain(|m| m.ts != ts);
        if let Some(t) = self.thread.as_mut().filter(|t| t.chan == ci) {
            t.msgs.retain(|m| m.ts != ts);
        }
        let n = self.view_msgs().len();
        if let Some(s) = self.sel {
            self.sel = if n == 0 { None } else { Some(s.min(n - 1)) };
        }
        self.anchor = self.anchor.filter(|&a| a < n);
    }

    fn new_msg(&mut self, ci: usize, m: Msg) {
        let me = self.me.clone();
        self.want_user(&m.user.clone());
        let reply = !m.thread_ts.is_empty() && m.thread_ts != m.ts && m.sub != "thread_broadcast";
        let mention = m.text.contains(&format!("<@{}>", me));
        if let Some(t) = self.thread.as_mut().filter(|t| t.chan == ci && t.ts == m.thread_ts) {
            if !t.msgs.iter().any(|x| x.ts == m.ts) {
                t.msgs.push(m.clone());
            }
        }
        let c = &mut self.chans[ci];
        if reply {
            if let Some(p) = c.msgs.iter_mut().find(|p| p.ts == m.thread_ts) {
                p.replies += 1;
            }
        } else if c.loaded && !c.msgs.iter().any(|x| x.ts == m.ts) {
            c.msgs.push(m.clone());
        }
        if !reply && ts_f(&m.ts) > ts_f(&c.latest) {
            c.latest = m.ts.clone();
        }
        let viewing = Some(ci) == self.cur;
        if m.user != me && (!viewing || reply) {
            if !reply && !viewing {
                c.unread = true;
            }
            if mention || (c.kind == Kind::Im && !viewing) {
                c.mentions += !viewing as u32;
                self.screen.raw(b"\x07");
            }
        }
        if viewing && !reply {
            self.mark_read(ci);
        }
    }

    fn refetch_chans(&mut self) {
        if self.last_chans.is_some_and(|t| t.elapsed() < CHANS_MIN) {
            return;
        }
        self.last_chans = Some(Instant::now());
        self.net.call_all(Tag::Chans, "users.conversations", &[("types", "public_channel,private_channel,mpim,im"), ("exclude_archived", "true"), ("limit", "200")], "channels", 50);
    }

    fn on_chans(&mut self, v: &Value) {
        let cur_id = self.cur.map(|c| self.chans[c].id.clone());
        let thread_id = self.thread.as_ref().map(|t| self.chans[t.chan].id.clone());
        let mut old: HashMap<String, Chan> = self.chans.drain(..).map(|c| (c.id.clone(), c)).collect();
        self.chans = v
            .get("channels")
            .arr()
            .iter()
            .map(|c| {
                let kind = if c.get("is_im").bool() {
                    Kind::Im
                } else if c.get("is_mpim").bool() {
                    Kind::Mpim
                } else if c.get("is_private").bool() {
                    Kind::Private
                } else {
                    Kind::Public
                };
                let mut name = c.get("name").str().to_string();
                if kind == Kind::Mpim {
                    name = name.trim_start_matches("mpdm-").rsplit_once('-').map_or(name.as_str(), |x| x.0).replace("--", ", ");
                }
                let user = c.get("user").opt_str().map(String::from);
                let id = c.get("id").str().to_string();
                match old.remove(&id) {
                    Some(o) => Chan { name, ..o },
                    None => Chan { id, name, kind, user, msgs: Vec::new(), loaded: false, more: true, unread: false, mentions: 0, latest: String::new(), seeded: false },
                }
            })
            .collect();
        self.chans.sort_by(|a, b| {
            let g = |k: Kind| matches!(k, Kind::Im | Kind::Mpim) as u8;
            g(a.kind).cmp(&g(b.kind)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.chan_names = self.chans.iter().filter(|c| matches!(c.kind, Kind::Public | Kind::Private)).map(|c| (c.id.clone(), c.name.clone())).collect();
        self.dm_names();
        if let Some(id) = cur_id {
            self.cur = self.chan_idx(&id);
            match (thread_id.and_then(|t| self.chan_idx(&t)), &mut self.thread) {
                (Some(i), Some(t)) => t.chan = i,
                _ => self.thread = None,
            }
            if self.cur.is_some() {
                return;
            }
        }
        let want = self.want.take().unwrap_or_else(|| "general".into());
        let w = want.trim_start_matches(['#', '@']).to_lowercase();
        let pick = self.chans.iter().position(|c| c.name.to_lowercase() == w || c.id == want).or_else(|| (!self.chans.is_empty()).then_some(0));
        if let Some(ci) = pick {
            self.open(ci);
        }
        self.info(format!("{}: {} conversations", self.team, self.chans.len()));
    }

    fn on_history(&mut self, id: &str, f: Fetch, v: &Value) {
        let Some(ci) = self.chan_idx(id) else { return };
        let me = self.me.clone();
        let mut fetched: Vec<Msg> = v.get("messages").arr().iter().map(|m| Msg::from(m, &me)).collect();
        fetched.reverse();
        let users: Vec<String> = fetched.iter().map(|m| m.user.clone()).collect();
        for u in users {
            self.want_user(&u);
        }
        let more = v.get("has_more").bool();
        let is_cur = Some(ci) == self.cur && self.thread.is_none();
        let c = &mut self.chans[ci];
        let had = c.msgs.len();
        match f {
            Fetch::Initial => {
                c.msgs = fetched;
                c.more = more;
                c.loaded = true;
            }
            Fetch::Older => {
                let n = fetched.len();
                fetched.append(&mut c.msgs);
                c.msgs = fetched;
                c.more = more;
                if is_cur {
                    self.sel = self.sel.map(|s| s + n);
                    self.pin = self.pin.map(|(m, o)| (m + n, o));
                    self.anchor = self.anchor.map(|a| a + n);
                }
            }
            Fetch::Newer => {
                let last = c.msgs.last().map(|m| ts_f(&m.ts)).unwrap_or(0.0);
                c.msgs.extend(fetched.into_iter().filter(|m| ts_f(&m.ts) > last));
            }
            Fetch::Refresh => {
                // Replace the window covered by the fetch: picks up edits,
                // reactions, reply counts and deletions.
                if let Some(lo) = fetched.first().map(|m| ts_f(&m.ts)) {
                    let keep = c.msgs.iter().position(|m| ts_f(&m.ts) >= lo).unwrap_or(c.msgs.len());
                    c.msgs.truncate(keep);
                    c.msgs.extend(fetched);
                    if is_cur {
                        if let Some(s) = self.sel {
                            self.sel = Some(s.min(c.msgs.len().saturating_sub(1)));
                        }
                    }
                }
            }
        }
        let c = &mut self.chans[ci];
        if let Some(m) = c.msgs.last() {
            if ts_f(&m.ts) > ts_f(&c.latest) {
                c.latest = m.ts.clone();
            }
        }
        c.seeded = true;
        let grew = c.msgs.len() > had && f != Fetch::Older;
        if f == Fetch::Older {
            let n = c.msgs.len() - had;
            self.info(if n == 0 { "no older messages".to_string() } else { format!("loaded {} older messages", n) });
        }
        if Some(ci) == self.cur && (f == Fetch::Initial || grew) {
            self.mark_read(ci);
        }
    }

    // ---------- actions ----------

    fn open(&mut self, ci: usize) {
        self.cur = Some(ci);
        self.thread = None;
        self.sel = None;
        self.pin = None;
        self.anchor = None;
        self.editing = None;
        self.last_poll = Instant::now();
        if self.chans[ci].loaded {
            self.load_history(ci, Fetch::Newer);
            self.mark_read(ci);
        } else {
            self.load_history(ci, Fetch::Initial);
        }
    }

    fn open_thread(&mut self, m: usize) {
        let (Some(ci), None) = (self.cur, &self.thread) else { return };
        let msg = &self.chans[ci].msgs[m];
        let ts = if msg.thread_ts.is_empty() { msg.ts.clone() } else { msg.thread_ts.clone() };
        self.thread = Some(Thread { chan: ci, ts, msgs: vec![msg.clone()] });
        self.sel = None;
        self.pin = None;
        self.anchor = None;
        self.load_thread();
    }

    fn close_thread(&mut self) {
        if let Some(t) = self.thread.take() {
            self.pin = None;
            self.anchor = None;
            self.sel = self.chans[t.chan].msgs.iter().position(|m| m.ts == t.ts);
        }
    }

    fn send(&mut self) {
        let Some(ci) = self.cur else { return };
        let raw = self.input.take();
        if raw.trim().is_empty() {
            return;
        }
        let text = format::encode(raw.trim_end(), &self.handles, &self.chans.iter().filter(|c| matches!(c.kind, Kind::Public | Kind::Private)).map(|c| (c.name.to_lowercase(), c.id.clone())).collect());
        let id = self.chans[ci].id.clone();
        if let Some(ts) = self.editing.take() {
            self.net.call(Tag::Done("edited"), "chat.update", &[("channel", &id), ("ts", &ts), ("text", &text)]);
            return;
        }
        let thread = self.thread.as_ref().map(|t| t.ts.clone());
        let mut p = vec![("channel", id.as_str()), ("text", text.as_str())];
        if let Some(t) = &thread {
            p.push(("thread_ts", t));
        }
        self.net.call(Tag::Posted(id.clone(), thread.clone()), "chat.postMessage", &p);
    }

    fn react(&mut self, name: &str) {
        let (Some(ci), Some(s)) = (self.cur, self.sel) else { return };
        let name = name.trim().trim_matches(':').to_string();
        if name.is_empty() {
            return;
        }
        let id = self.chans[ci].id.clone();
        let msgs = match &mut self.thread {
            Some(t) => &mut t.msgs,
            None => &mut self.chans[ci].msgs,
        };
        let Some(m) = msgs.get_mut(s) else { return };
        let ts = m.ts.clone();
        let mine = m.reactions.iter().position(|r| r.0 == name && r.2);
        match mine {
            Some(i) => {
                m.reactions[i].1 -= 1;
                m.reactions[i].2 = false;
                m.reactions.retain(|r| r.1 > 0);
                self.net.call(Tag::Done("reaction removed"), "reactions.remove", &[("channel", &id), ("timestamp", &ts), ("name", &name)]);
            }
            None => {
                match m.reactions.iter_mut().find(|r| r.0 == name) {
                    Some(r) => {
                        r.1 += 1;
                        r.2 = true;
                    }
                    None => m.reactions.push((name.clone(), 1, true)),
                }
                self.net.call(Tag::Done("reacted"), "reactions.add", &[("channel", &id), ("timestamp", &ts), ("name", &name)]);
            }
        }
    }

    fn complete(&mut self) {
        if let Some((start, cands, idx)) = &mut self.comp {
            if !cands.is_empty() {
                *idx = (*idx + 1) % cands.len();
                let s = cands[*idx].clone();
                self.input.replace(*start, &s);
            }
            return;
        }
        let (start, word) = self.input.word_before(&['@', '#', ':', '+']);
        let start = start + word.find(['@', '#', ':']).unwrap_or(0);
        let word = &self.input.text[start..self.input.cur];
        let (sigil, q) = match word.chars().next() {
            Some(c @ ('@' | '#' | ':')) => (c, &word[1..]),
            _ => return,
        };
        let pool: Vec<String> = match sigil {
            '@' => {
                let mut v: Vec<String> = self.handles.keys().cloned().collect();
                v.extend(["here", "channel"].map(String::from));
                v.sort();
                v
            }
            '#' => self.chan_names.values().cloned().collect(),
            _ => format::EMOJI.iter().map(|e| e.0.to_string()).collect(),
        };
        let cands: Vec<String> = fuzzy::filter(q, pool.iter().map(|s| s.as_str()))
            .into_iter()
            .take(50)
            .map(|i| if sigil == ':' { format!(":{}: ", pool[i]) } else { format!("{}{} ", sigil, pool[i]) })
            .collect();
        if let Some(s) = cands.first() {
            let s = s.clone();
            self.input.replace(start, &s);
            if cands.len() > 1 {
                self.info(format!("{} matches (Tab for next)", cands.len()));
            }
            self.comp = Some((start, cands, 0));
        }
    }

    fn open_url(&mut self, u: &str) {
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let r = std::process::Command::new(cmd).arg(u).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        match r {
            Ok(_) => self.info(format!("opened {}", u)),
            Err(e) => self.error(format!("{}: {}", cmd, e)),
        }
    }

    fn move_sel(&mut self, d: isize) {
        self.pin = None;
        let n = self.view_msgs().len();
        if n == 0 {
            return;
        }
        let s = self.sel.unwrap_or(n) as isize + d;
        if s < 0 {
            self.sel = Some(0);
            self.older();
        } else if s as usize >= n {
            self.sel = if self.anchor.is_some() { Some(n - 1) } else { None };
        } else {
            self.sel = Some(s as usize);
        }
    }

    fn older(&mut self) {
        if let (Some(ci), None) = (self.cur, &self.thread) {
            if self.chans[ci].more {
                self.info("loading older messages…");
                self.load_history(ci, Fetch::Older);
            }
        }
    }

    fn switch_rel(&mut self, d: isize, unread_only: bool) {
        self.side_top = None;
        let n = self.chans.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.cur.map_or(0, |c| c as isize);
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if !unread_only || self.chans[i as usize].unread {
                self.open(i as usize);
                return;
            }
        }
        self.info("no unread conversations");
    }

    fn chan_label(c: &Chan) -> String {
        match c.kind {
            Kind::Public => format!("#{}", c.name),
            Kind::Private => format!("🔒{}", c.name),
            Kind::Im => format!("@{}", c.name),
            Kind::Mpim => format!("👥{}", c.name),
        }
    }

    // ---------- keys ----------

    pub fn handle_key(&mut self, k: Key) {
        if k == Key::Ctrl('c') && matches!(self.mode, Mode::Normal) {
            self.quit = true;
            return;
        }
        if k == Key::Ctrl('z') {
            self.suspend = true;
            return;
        }
        if k == Key::Ctrl('l') {
            self.screen.invalidate();
            return;
        }
        if k == Key::Ctrl('k') && matches!(self.mode, Mode::Normal | Mode::Insert) {
            let items = self.chans.iter().enumerate().map(|(i, c)| ChanItem(format!("{}{}", Self::chan_label(c), if c.unread { " •" } else { "" }), i)).collect();
            self.mode = Mode::Pick(Picker::new("switch", items));
            return;
        }
        if let Key::Mouse(m) = k {
            match &self.mode {
                Mode::Normal | Mode::Insert => return self.mouse(m),
                Mode::Help | Mode::ConfirmDelete(_) if matches!(m.kind, MouseKind::Press(_)) => {
                    self.mode = Mode::Normal;
                    return;
                }
                Mode::Pick(_) | Mode::Links(_) => {}
                _ => return,
            }
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.normal_key(k),
            Mode::Insert => {
                self.mode = Mode::Insert;
                self.insert_key(k);
            }
            Mode::Pick(mut p) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Pick(p),
                Pick::Cancel => {}
                Pick::Accept => {
                    if let Some(it) = p.selected() {
                        self.open(it.1);
                    }
                }
            },
            Mode::Links(mut p) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Links(p),
                Pick::Cancel => {}
                Pick::Accept => {
                    if let Some(u) = p.selected().cloned() {
                        self.open_url(&u);
                    }
                }
            },
            Mode::React(mut e) => match k {
                Key::Esc | Key::Ctrl('c') => {}
                Key::Enter => self.react(&e.text.clone()),
                Key::Tab => {
                    let q = e.text.trim_matches(':').to_string();
                    let names: Vec<&str> = format::EMOJI.iter().map(|x| x.0).collect();
                    if let Some(&i) = fuzzy::filter(&q, names.iter().copied()).first() {
                        e.set(names[i]);
                    }
                    self.mode = Mode::React(e);
                }
                k => {
                    e.key(&k);
                    self.mode = Mode::React(e);
                }
            },
            Mode::ConfirmDelete(ts) => {
                if k == Key::Char('y') {
                    if let Some(ci) = self.cur {
                        let id = self.chans[ci].id.clone();
                        self.net.call(Tag::Done("deleted"), "chat.delete", &[("channel", &id), ("ts", &ts)]);
                    }
                } else {
                    self.info("");
                }
            }
            Mode::Help => {}
        }
    }

    fn insert_key(&mut self, k: Key) {
        if k != Key::Tab {
            self.comp = None;
        }
        match k {
            Key::Esc => {
                self.mode = Mode::Normal;
                if self.editing.take().is_some() {
                    self.input.clear();
                }
            }
            Key::Enter => self.send(),
            Key::Tab => self.complete(),
            Key::PageUp => self.move_sel(-5),
            Key::PageDown => self.move_sel(5),
            Key::Up if self.input.text.is_empty() => self.edit_last(),
            k => {
                if let Edit::Ignored = self.input.key(&k) {
                    self.normal_key_in_insert(k);
                }
            }
        }
    }

    /// Non-editing keys while composing: Ctrl-N/P browse messages.
    fn normal_key_in_insert(&mut self, k: Key) {
        match k {
            Key::Ctrl('p') => self.move_sel(-1),
            Key::Ctrl('n') => self.move_sel(1),
            _ => {}
        }
    }

    /// Up in an empty composer edits your last message, like Slack.
    fn edit_last(&mut self) {
        let me = self.me.clone();
        if let Some(i) = self.view_msgs().iter().rposition(|m| m.user == me) {
            self.sel = Some(i);
            self.start_edit();
        }
    }

    fn start_edit(&mut self) {
        let Some(s) = self.sel else { return };
        let Some(m) = self.view_msgs().get(s) else { return };
        if m.user != self.me {
            return self.error("can only edit your own messages");
        }
        let (ts, raw) = (m.ts.clone(), m.text.clone());
        let n = Names { users: &self.users, chans: &self.chan_names, me: &self.me };
        let plain = format::render(&raw, &n).text;
        self.input.set(&plain);
        self.editing = Some(ts);
        self.mode = Mode::Insert;
        self.info("editing message (Esc to cancel)");
    }

    fn normal_key(&mut self, k: Key) {
        if self.anchor.is_some() {
            match k {
                Key::Esc | Key::Char('v') | Key::Char('V') => {
                    self.anchor = None;
                    return self.info("");
                }
                Key::Char('y') => {
                    let (a, b) = self.range().unwrap();
                    let t = self.transcript(a, b);
                    self.screen.osc52(&t);
                    self.anchor = None;
                    return self.info(format!("copied {} message{}", b - a + 1, if a == b { "" } else { "s" }));
                }
                Key::Char('G') | Key::End => {
                    let n = self.view_msgs().len();
                    self.sel = n.checked_sub(1);
                    return;
                }
                Key::Char('j' | 'k' | 'g') | Key::Up | Key::Down | Key::Ctrl('d' | 'u') | Key::PageUp | Key::PageDown => {}
                _ => self.anchor = None,
            }
        }
        let g = std::mem::take(&mut self.pending_g);
        let half = (self.screen.h / 2).max(1) as isize;
        match k {
            Key::Char('q') if self.thread.is_some() => self.close_thread(),
            Key::Char('q') => self.quit = true,
            Key::Esc | Key::Char('h') | Key::Left if self.thread.is_some() => self.close_thread(),
            Key::Esc => (self.sel, self.pin) = (None, None),
            Key::Char('i') | Key::Char('a') => self.mode = Mode::Insert,
            Key::Char('j') | Key::Down => self.move_sel(1),
            Key::Char('k') | Key::Up => self.move_sel(-1),
            Key::Ctrl('d') | Key::PageDown => self.scroll_lines(half),
            Key::Ctrl('u') | Key::PageUp => self.scroll_lines(-half),
            Key::Char('G') | Key::End => (self.sel, self.pin) = (None, None),
            Key::Char('g') if g => {
                self.sel = Some(0);
                self.older();
            }
            Key::Char('g') => self.pending_g = true,
            Key::Char('x') if g => {
                let Some(m) = self.sel.and_then(|s| self.view_msgs().get(s)) else { return };
                let u = urls(&m.text);
                match u.len() {
                    0 => self.info("no links in this message"),
                    1 => self.open_url(&u[0]),
                    _ => self.mode = Mode::Links(Picker::new("links", u)),
                }
            }
            Key::Enter | Key::Char('l') | Key::Right | Key::Char('t') | Key::Char('r') => {
                let had_sel = self.sel.is_some();
                if let (Some(s), None) = (self.sel, &self.thread) {
                    self.open_thread(s);
                }
                if k == Key::Char('r') || (k == Key::Enter && !had_sel) {
                    self.mode = Mode::Insert;
                }
            }
            Key::Char('J') | Key::Tab => self.switch_rel(1, false),
            Key::Char('K') | Key::BackTab => self.switch_rel(-1, false),
            Key::Char('u') => self.switch_rel(1, true),
            Key::Char('b') => self.sidebar = !self.sidebar,
            Key::Char('+') if self.sel.is_some() => self.mode = Mode::React(LineEdit::new()),
            Key::Char('e') => self.start_edit(),
            Key::Char('D') => {
                let Some(m) = self.sel.and_then(|s| self.view_msgs().get(s)) else { return };
                if m.user != self.me {
                    return self.error("can only delete your own messages");
                }
                self.mode = Mode::ConfirmDelete(m.ts.clone());
            }
            Key::Char('y') => {
                let Some(m) = self.sel.and_then(|s| self.view_msgs().get(s)) else { return };
                let n = Names { users: &self.users, chans: &self.chan_names, me: &self.me };
                let t = format::render(&m.text, &n).text;
                self.screen.osc52(&t);
                self.info("copied");
            }
            Key::Char('R') => {
                if let Some(ci) = self.cur {
                    self.load_history(ci, Fetch::Refresh);
                }
                self.load_thread();
                self.last_counts = Instant::now() - COUNTS;
                self.info("refreshing…");
            }
            Key::Char('v') | Key::Char('V') => {
                let n = self.view_msgs().len();
                if n > 0 {
                    let s = self.sel.unwrap_or(n - 1);
                    (self.sel, self.anchor) = (Some(s), Some(s));
                    self.info("visual: j/k to extend, y to copy, Esc to cancel");
                }
            }
            Key::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
    }

    fn scroll_lines(&mut self, d: isize) {
        self.pin = None;
        if self.rows_cache.is_empty() {
            return;
        }
        let n = self.view_msgs().len();
        let cur_row = match self.sel {
            Some(s) => self.rows_cache.iter().position(|&m| m == s).unwrap_or(0),
            None => self.rows_cache.len() - 1,
        };
        let r = (cur_row as isize + d).clamp(0, self.rows_cache.len() as isize - 1) as usize;
        let m = self.rows_cache[r];
        if d > 0 && (r + 1 >= self.rows_cache.len() || m >= n) {
            self.sel = if self.anchor.is_some() { n.checked_sub(1) } else { None };
        } else if m < n {
            self.sel = Some(m);
            if r == 0 {
                self.older();
            }
        }
    }

    // ---------- mouse ----------

    fn mouse(&mut self, m: Mouse) {
        let g = &self.geo;
        let in_side = m.x < g.side_w;
        let in_comp = m.y >= g.y_comp && m.y < g.y_comp + g.comp_rows && !in_side;
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let d = if m.kind == MouseKind::WheelUp { -3 } else { 3 };
                if in_side {
                    let max = self.chans.len().saturating_sub(self.screen.h.saturating_sub(2));
                    self.side_top = Some((self.geo.side_start as isize + d).clamp(0, max as isize) as usize);
                } else {
                    self.scroll_view(d);
                }
            }
            MouseKind::Press(0) => {
                if in_side {
                    let i = g.side_start + m.y.saturating_sub(1);
                    if m.y >= 1 && i < self.chans.len() {
                        self.open(i);
                    }
                } else if m.y == 0 {
                    self.close_thread();
                } else if in_comp {
                    let (x, y) = (m.x.saturating_sub(g.comp_x), m.y - g.y_comp);
                    if !self.input.text.is_empty() {
                        self.input.cur = self.input.offset_at(g.comp_w, g.comp_rows, x, y);
                    }
                    self.mode = Mode::Insert;
                } else if let Some(i) = g.hits.iter().position(|(y, x0, x1, _)| *y == m.y && (*x0..*x1).contains(&m.x)) {
                    match &self.geo.hits[i].3 {
                        Hit::Link(u) => {
                            let u = u.clone();
                            self.open_url(&u);
                        }
                        Hit::React(mi, name) => {
                            let (mi, name) = (*mi, name.clone());
                            self.sel = Some(mi);
                            self.react(&name);
                        }
                        Hit::Thread(mi) => {
                            let mi = *mi;
                            self.open_thread(mi);
                        }
                    }
                } else if let Some(Some(mi)) = g.row_msg.get(m.y).copied() {
                    // click selects; a second click on the same message opens its thread
                    let double = self.last_click.is_some_and(|(t, p)| p == mi && t.elapsed() < Duration::from_millis(400));
                    self.sel = Some(mi);
                    self.anchor = None;
                    self.drag_from = Some(mi);
                    self.last_click = Some((Instant::now(), mi));
                    if double && self.thread.is_none() {
                        self.last_click = None;
                        self.open_thread(mi);
                    }
                }
            }
            MouseKind::Drag(0) => {
                let Some(from) = self.drag_from else { return };
                // dragging past the top or bottom of the pane scrolls
                let (top, bot) = (1, self.geo.y_comp.saturating_sub(1));
                if m.y < top {
                    self.scroll_view(-1);
                } else if m.y >= bot {
                    self.scroll_view(1);
                }
                let y = m.y.clamp(top, bot.saturating_sub(1));
                // on a date divider or padding: use the nearest message row
                let at = |yy: usize| (top..bot).contains(&yy).then(|| self.geo.row_msg.get(yy).copied().flatten()).flatten();
                let row = (0..bot).find_map(|d| at(y + d).or_else(|| y.checked_sub(d).and_then(at)));
                if let Some(mi) = row {
                    self.anchor = Some(from);
                    self.sel = Some(mi);
                    self.last_click = None;
                }
            }
            MouseKind::Release => {
                self.drag_from = None;
                if let Some((a, b)) = self.range().filter(|(a, b)| a != b) {
                    self.info(format!("{} messages selected: y to copy, Esc to cancel", b - a + 1));
                } else {
                    self.anchor = None;
                }
            }
            _ => {}
        }
    }

    fn is_selected(&self, m: usize) -> bool {
        match self.range() {
            Some((a, b)) => (a..=b).contains(&m),
            None => self.sel == Some(m),
        }
    }

    /// Scroll the message pane by `d` rows without moving the selection.
    fn scroll_view(&mut self, d: isize) {
        let total = self.rows_cache.len();
        if total == 0 {
            return;
        }
        let max_top = total.saturating_sub(self.geo.view_h);
        let t = (self.top as isize + d).clamp(0, max_top as isize) as usize;
        if d > 0 && t >= max_top {
            // reached the bottom: follow new messages again
            (self.pin, self.sel, self.top) = (None, None, max_top);
            return;
        }
        // update now: a burst of wheel events arrives before the next render
        self.top = t;
        let m = self.rows_cache[t];
        let first = self.rows_cache.iter().position(|&x| x == m).unwrap_or(t);
        self.pin = Some((m, t - first));
        if t == 0 && d < 0 {
            self.older();
        }
    }

    // ---------- drawing ----------

    fn build_rows(&self, msgs: &[Msg], tw: usize, parent: bool) -> (Vec<Row>, Vec<Rich>) {
        let n = Names { users: &self.users, chans: &self.chan_names, me: &self.me };
        let mut rows = Vec::new();
        let mut riches = Vec::with_capacity(msgs.len());
        let mut prev: Option<(&Msg, i32)> = None;
        let now = local_tm(&format!("{}", unsafe { libc::time(std::ptr::null_mut()) }));
        let today = now.tm_year * 400 + now.tm_yday;
        for (i, m) in msgs.iter().enumerate() {
            let tm = local_tm(&m.ts);
            let day = tm.tm_year * 400 + tm.tm_yday;
            if prev.map_or(true, |p| p.1 != day) {
                rows.push(Row::Day(match today - day {
                    0 => "Today".into(),
                    1 => "Yesterday".into(),
                    _ => day_label(&tm),
                }));
            }
            let mut rich = format::render(&m.text, &n);
            if !m.sub.is_empty() && m.sub != "thread_broadcast" && m.sub != "bot_message" && m.sub != "file_share" && m.sub != "me_message" {
                rich.spans.fill(Span::Italic);
            }
            if m.edited {
                rich.text.push_str(" (edited)");
                rich.spans.extend(std::iter::repeat(Span::Quote).take(9));
            }
            let head = prev.map_or(true, |(p, d)| p.user != m.user || p.bot != m.bot || d != day || ts_f(&m.ts) - ts_f(&p.ts) > 300.0) || (parent && i == 1);
            for (a, b) in wrap(&rich.text, tw) {
                rows.push(Row::Text { m: i, head: head && rows.last().map_or(true, |r| !matches!(r, Row::Text { m: mm, .. } if *mm == i)), a, b });
            }
            for f in m.files.iter().filter(|f| !m.images.iter().any(|p| &p.name == *f)) {
                rows.push(Row::Extra { m: i, s: format!("📎 {}", f), kind: Extra::Files });
            }
            for (k, p) in m.images.iter().enumerate() {
                let of = self.gallery.size(&p.url, tw.min(60), 16).map_or(1, |s| s.1);
                for li in 0..of {
                    rows.push(Row::Img { m: i, k, i: li, of });
                }
            }
            if !m.reactions.is_empty() {
                let s = m.reactions.iter().map(chip).collect::<Vec<_>>().join(" ");
                rows.push(Row::Extra { m: i, s, kind: Extra::Reactions });
            }
            if m.replies > 0 && !(parent && i == 0) {
                rows.push(Row::Extra { m: i, s: format!("↳ {} {}", m.replies, if m.replies == 1 { "reply" } else { "replies" }), kind: Extra::Replies });
            }
            riches.push(rich);
            prev = Some((m, day));
        }
        (rows, riches)
    }

    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if w < 20 || h < 5 {
            self.screen.puts(0, 0, "too small", Style::default(), w);
            self.screen.flush(None);
            return;
        }
        let side_w = if self.sidebar && w >= 60 { (w / 5).clamp(16, 28) } else { 0 };
        let x0 = if side_w > 0 { side_w + 1 } else { 0 };
        let pw = w - x0;

        // composer + status
        let prompt = if self.editing.is_some() { "edit› " } else if self.thread.is_some() { "reply› " } else { "› " };
        let pwid = str_width(prompt);
        let ch = self.input.height(pw.saturating_sub(1), pwid).clamp(1, (h / 3).max(1));
        let y_status = h - 1;
        let y_comp = y_status - ch;
        let insert = matches!(self.mode, Mode::Insert);
        let comp_st = Style::new(if insert { 255 } else { 248 }, 0, 0);
        self.screen.fill(x0, w, y_comp.saturating_sub(1), Style::new(238, 0, 0));
        for x in x0..w {
            self.screen.put(x, y_comp - 1, '─', Style::fg(238));
        }
        self.screen.puts(x0, y_comp, prompt, Style::new(if insert { ACCENT } else { FG_DIM }, 0, BOLD), w);
        let mut cursor = None;
        self.geo.side_w = side_w;
        (self.geo.y_comp, self.geo.comp_rows, self.geo.comp_x, self.geo.comp_w) = (y_comp, ch, x0 + pwid, pw.saturating_sub(pwid + 1));
        if self.input.text.is_empty() && !insert {
            let target = self.cur.map(|c| Self::chan_label(&self.chans[c])).unwrap_or_default();
            let hint = if self.thread.is_some() { "reply in thread (i)".to_string() } else { format!("message {} (i)", target) };
            self.screen.puts(x0 + pwid, y_comp, &hint, Style::new(FG_DIM, 0, ITALIC), w);
        } else {
            let (cx, cy) = self.input.draw(&mut self.screen, x0 + pwid, y_comp, pw.saturating_sub(pwid + 1), ch, comp_st);
            if insert {
                cursor = Some((cx, cy, true));
            }
        }

        // header
        let y_top = 1;
        let y_bot = y_comp - 1; // exclusive
        let title = match (&self.thread, self.cur) {
            (Some(t), _) => format!(" thread in {} ", Self::chan_label(&self.chans[t.chan])),
            (None, Some(c)) => format!(" {} ", Self::chan_label(&self.chans[c])),
            _ => " mslack ".into(),
        };
        self.screen.fill(x0, w, 0, Style::new(252, BG_BAR, 0));
        self.screen.puts(x0, 0, &title, Style::new(255, BG_BAR, BOLD), w);

        // messages
        let narrow = pw < 70;
        let name_w = if narrow { 10 } else { 14 };
        let tx = x0 + 1 + 6 + name_w + 1;
        let tw = w.saturating_sub(tx + 1).max(10);
        let msgs: Vec<Msg> = self.view_msgs().to_vec();
        let (rows, riches) = self.build_rows(&msgs, tw, self.thread.is_some());
        self.rows_cache = rows.iter().map(|r| match r {
            Row::Day(_) => usize::MAX,
            Row::Text { m, .. } | Row::Extra { m, .. } | Row::Img { m, .. } => *m,
        }).collect();
        // fill Day rows with the following message so scrolling works
        for i in (0..self.rows_cache.len()).rev() {
            if self.rows_cache[i] == usize::MAX {
                self.rows_cache[i] = self.rows_cache.get(i + 1).copied().unwrap_or(usize::MAX);
            }
        }
        let view_h = y_bot.saturating_sub(y_top);
        let total = rows.len();
        self.geo.view_h = view_h;
        match (self.pin, self.sel) {
            (Some((pm, off)), _) => {
                let first = self.rows_cache.iter().position(|&m| m == pm).unwrap_or(0);
                self.top = (first + off).min(total.saturating_sub(view_h));
            }
            (None, None) => self.top = total.saturating_sub(view_h),
            (None, Some(s)) => {
                let first = rows.iter().position(|r| matches!(r, Row::Text { m, .. } | Row::Extra { m, .. } | Row::Img { m, .. } if *m == s)).unwrap_or(0);
                let first = if first > 0 && matches!(rows[first - 1], Row::Day(_)) { first - 1 } else { first };
                let last = rows.iter().rposition(|r| matches!(r, Row::Text { m, .. } | Row::Extra { m, .. } | Row::Img { m, .. } if *m == s)).unwrap_or(first);
                if last >= self.top + view_h {
                    self.top = last + 1 - view_h;
                }
                if first < self.top {
                    self.top = first;
                }
                self.top = self.top.min(total.saturating_sub(view_h));
            }
        }
        // bottom-anchor short conversations
        let pad = view_h.saturating_sub(total);
        if total == 0 {
            let s = match self.cur {
                Some(c) if !self.chans[c].loaded => "loading…",
                Some(_) => "no messages",
                None => "",
            };
            self.screen.puts(x0 + 2, y_top + view_h / 2, s, Style::new(FG_DIM, 0, ITALIC), w);
        }
        self.geo.row_msg = vec![None; h];
        self.geo.hits.clear();
        // visible span of each ready image: screen row of its first visible
        // line, first and one-past-last line shown, and total lines
        let mut pics: HashMap<(usize, usize), (usize, usize, usize, usize)> = HashMap::new();
        for (k, row) in rows.iter().enumerate().skip(self.top).take(view_h) {
            let y = y_top + pad + k - self.top;
            if let Row::Text { m, .. } | Row::Extra { m, .. } | Row::Img { m, .. } = row {
                self.geo.row_msg[y] = Some(*m);
            }
            match row {
                Row::Day(d) => {
                    let s = format!(" {} ", d);
                    let mid = x0 + pw.saturating_sub(str_width(&s)) / 2;
                    for x in x0 + 1..w - 1 {
                        self.screen.put(x, y, '─', Style::fg(237));
                    }
                    self.screen.puts(mid, y, &s, Style::new(FG_DIM, 0, 0), w);
                }
                Row::Text { m, head, a, b } => {
                    let msg = &msgs[*m];
                    let selected = self.is_selected(*m);
                    let bg = if selected { BG_SEL } else { 0 };
                    if selected {
                        self.screen.fill(x0, w, y, Style::new(0, bg, 0));
                        self.screen.put(x0, y, '▌', Style::new(ACCENT, bg, 0));
                    }
                    if *head {
                        let tm = local_tm(&msg.ts);
                        self.screen.puts(x0 + 1, y, &format!("{:02}:{:02}", tm.tm_hour, tm.tm_min), Style::new(FG_DIM, bg, 0), w);
                        let name = self.name_of(msg);
                        let name: String = name.chars().take(name_w).collect();
                        let nx = x0 + 7 + name_w - str_width(&name);
                        self.screen.puts(nx, y, &name, Style::new(name_color(&msg.user), bg, BOLD), w);
                    }
                    let rich = &riches[*m];
                    let mut x = tx;
                    for (off, c) in rich.text[*a..*b].char_indices() {
                        let st = match rich.spans[a + off] {
                            Span::Plain => Style::new(0, bg, 0),
                            Span::Mention => Style::new(C_MENTION, bg, 0),
                            Span::MentionMe => Style::new(C_ME, bg, BOLD),
                            Span::Link => Style::new(C_LINK, bg, UNDERLINE),
                            Span::Code => Style::new(C_CODE, bg, 0),
                            Span::Bold => Style::new(0, bg, BOLD),
                            Span::Italic => Style::new(0, bg, ITALIC),
                            Span::Strike => Style::new(FG_DIM, bg, 0),
                            Span::Quote => Style::new(FG_DIM, bg, ITALIC),
                        };
                        if x >= w {
                            break;
                        }
                        let x1 = x + self.screen.put(x, y, c, st);
                        if let Some((_, _, u)) = rich.links.iter().find(|(la, lb, _)| (*la..*lb).contains(&(a + off))) {
                            match self.geo.hits.last_mut() {
                                Some((hy, _, hx1, Hit::Link(hu))) if *hy == y && *hx1 == x && hu == u => *hx1 = x1,
                                _ => self.geo.hits.push((y, x, x1, Hit::Link(u.clone()))),
                            }
                        }
                        x = x1;
                    }
                }
                Row::Img { m, k: pk, i, of } => {
                    let selected = self.is_selected(*m);
                    let bg = if selected { BG_SEL } else { 0 };
                    if selected {
                        self.screen.fill(x0, w, y, Style::new(0, bg, 0));
                        self.screen.put(x0, y, '▌', Style::new(ACCENT, bg, 0));
                    }
                    let p = &msgs[*m].images[*pk];
                    if self.gallery.size(&p.url, tw.min(60), 16).is_some() {
                        let e = pics.entry((*m, *pk)).or_insert((y, *i, *i, *of));
                        e.2 = i + 1;
                    } else {
                        if self.gallery.request(&p.url) {
                            self.net.fetch_image(&p.url, &p.url);
                        }
                        let note = match self.gallery.slot(&p.url) {
                            Some(mimg::Slot::Failed(e)) => format!(" ({})", e),
                            Some(mimg::Slot::Loading) => " …".to_string(),
                            _ => String::new(),
                        };
                        self.screen.puts(tx, y, &format!("📎 {}{}", p.name, note), Style::new(250, bg, 0), w);
                    }
                }
                Row::Extra { m, s, kind } => {
                    let selected = self.is_selected(*m);
                    let bg = if selected { BG_SEL } else { 0 };
                    if selected {
                        self.screen.fill(x0, w, y, Style::new(0, bg, 0));
                        self.screen.put(x0, y, '▌', Style::new(ACCENT, bg, 0));
                    }
                    let fg = if *kind == Extra::Replies { C_LINK } else { 250 };
                    let end = self.screen.puts(tx, y, s, Style::new(fg, bg, 0), w);
                    match kind {
                        Extra::Replies => self.geo.hits.push((y, tx, end, Hit::Thread(*m))),
                        Extra::Reactions => {
                            let mut x = tx;
                            for r in &msgs[*m].reactions {
                                let cw = str_width(&chip(r));
                                self.geo.hits.push((y, x, x + cw, Hit::React(*m, r.0.clone())));
                                x += cw + 1;
                            }
                        }
                        Extra::Files => {}
                    }
                }
            }
        }
        let overlay = matches!(self.mode, Mode::Pick(_) | Mode::Links(_) | Mode::Help) || self.comp.is_some();
        let mut ims = Vec::new();
        if !overlay {
            for ((m, k), (y, i0, i1, of)) in pics {
                let p = &msgs[m].images[k];
                if let Some((cols, _)) = self.gallery.size(&p.url, tw.min(60), 16) {
                    ims.extend(self.gallery.place(&p.url, (m * 4 + k + 1) as u32, (tx, y), cols, of, i0, i1));
                }
            }
        }
        self.screen.set_images(ims);
        if self.top > 0 || self.cur.is_some_and(|c| self.chans[c].more && self.thread.is_none() && self.chans[c].loaded && self.top == 0 && self.sel == Some(0)) {
            let s = if self.top > 0 { format!("↑{} ", self.top) } else { "gg/k: older ".into() };
            self.screen.puts(w.saturating_sub(str_width(&s)), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }

        // sidebar
        if side_w > 0 {
            for y in 0..h - 1 {
                self.screen.fill(0, side_w, y, Style::new(0, BG_SIDE, 0));
                self.screen.put(side_w, y, '│', Style::fg(237));
            }
            self.screen.puts(1, 0, &self.team, Style::new(ACCENT, BG_SIDE, BOLD), side_w);
            let list_h = h - 2;
            let cur = self.cur.unwrap_or(0);
            let start = self.side_top.unwrap_or_else(|| (cur + 1).saturating_sub(list_h).max(cur.saturating_sub(list_h / 2))).min(self.chans.len().saturating_sub(list_h));
            self.geo.side_start = start;
            for (k, c) in self.chans.iter().enumerate().skip(start).take(list_h) {
                let y = 1 + k - start;
                let is_cur = Some(k) == self.cur;
                let bg = if is_cur { BG_SEL } else { BG_SIDE };
                let st = Style::new(if c.unread { 255 } else { 245 }, bg, if c.unread || is_cur { BOLD } else { 0 });
                self.screen.fill(0, side_w, y, Style::new(0, bg, 0));
                let x = self.screen.puts(1, y, &Self::chan_label(c), st, side_w - 1);
                if c.mentions > 0 {
                    let s = format!(" {}", c.mentions);
                    self.screen.puts((side_w - 1 - s.len()).max(x), y, &s, Style::new(C_ME, bg, BOLD), side_w);
                }
            }
        }

        // status line
        let (tag, tag_bg) = match &self.mode {
            Mode::Insert => (if self.editing.is_some() { " EDIT " } else { " INSERT " }, 108),
            Mode::Pick(_) | Mode::Links(_) => (" PICK ", 180),
            Mode::React(_) => (" REACT ", 176),
            Mode::ConfirmDelete(_) => (" DELETE ", 203),
            _ if self.thread.is_some() => (" THREAD ", 110),
            _ if self.anchor.is_some() => (" VISUAL ", 176),
            _ => (" NORMAL ", 110),
        };
        self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
        let mut x = self.screen.puts(0, y_status, tag, Style::new(16, tag_bg, BOLD), w);
        x += 1;
        match &self.mode {
            Mode::React(e) => {
                let x2 = self.screen.puts(x, y_status, "react :", Style::new(ACCENT, BG_BAR, 0), w);
                let (cx, cy) = e.draw(&mut self.screen, x2, y_status, w - x2, 1, Style::new(255, BG_BAR, 0));
                cursor = Some((cx, cy, true));
            }
            Mode::ConfirmDelete(_) => {
                self.screen.puts(x, y_status, "delete this message? (y/n)", Style::new(C_ERR, BG_BAR, BOLD), w);
            }
            _ => {
                let st = Style::new(if self.err { C_ERR } else { 250 }, BG_BAR, 0);
                self.screen.puts(x, y_status, &self.msg.lines().next().unwrap_or("").to_string(), st, w);
            }
        }
        let unread = self.chans.iter().filter(|c| c.unread).count();
        let link = match (self.socket, self.live) {
            (_, true) => "● live  ",
            (true, false) => "○ polling  ",
            _ => "",
        };
        let right = format!("{}{}{}? help ", if self.inflight.is_empty() { "" } else { "… " }, if unread > 0 { format!("{} unread  ", unread) } else { String::new() }, link);
        self.screen.puts(w.saturating_sub(str_width(&right)), y_status, &right, Style::new(FG_DIM, BG_BAR, 0), w);

        // overlays
        match &mut self.mode {
            Mode::Pick(p) => {
                let (x, y) = p.draw(&mut self.screen, y_status);
                cursor = Some((x, y, true));
            }
            Mode::Links(p) => {
                let (x, y) = p.draw(&mut self.screen, y_status);
                cursor = Some((x, y, true));
            }
            Mode::Help => {
                let lines: Vec<&str> = HELP.lines().collect();
                let bw = lines.iter().map(|l| str_width(l)).max().unwrap_or(0) + 4;
                let bx = w.saturating_sub(bw) / 2;
                let by = h.saturating_sub(lines.len() + 2) / 2;
                for (i, l) in std::iter::once("").chain(lines.iter().copied()).chain(std::iter::once("")).enumerate() {
                    self.screen.fill(bx, bx + bw, by + i, Style::new(252, BG_BAR, 0));
                    let st = if l.ends_with(':') { Style::new(ACCENT, BG_BAR, BOLD) } else { Style::new(252, BG_BAR, 0) };
                    self.screen.puts(bx + 2, by + i, l, st, bx + bw);
                }
                cursor = None;
            }
            _ => {}
        }
        self.screen.flush(cursor);
    }
}

pub const HELP: &str = "\
normal mode:
  j k ↑ ↓      select message      G  back to latest
  C-d C-u      half page           gg oldest (loads more)
  Enter l t    open thread         r  reply in thread
  h q Esc      leave thread        q  quit
  i a          compose             e  edit own message
  +            react / unreact     D  delete own message
  y            copy text (OSC 52)  gx open link
  v            select a range (j/k extend, y copies them as a transcript)
  Tab J / K    next / prev chan    u  next unread
  C-k          channel picker      b  toggle sidebar
  R            refresh             C-z suspend
mouse:
  click        select / open chan  double-click  open thread
  drag         select several messages, then y to copy
  wheel        scroll              click link, reaction, ↳ replies
  shift+drag   select text (terminal)
compose:
  Enter        send                Alt-Enter newline
  Tab          complete @user #chan :emoji:
  ↑ (empty)    edit last message   Esc  normal mode
  C-a C-e C-w C-u C-k  readline-style editing";
