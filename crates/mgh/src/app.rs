// mgh state, keys and drawing.

use crate::api::{self, Net, Reply, Req, Tag};
use mtui::fuzzy;
use mtui::json::{self, Value};
use mtui::lineedit::LineEdit;
use mtui::picker::{Pick, Picker};
use mtui::screen::{Screen, Style, BOLD, ITALIC, };
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::{str_width, wrap};
use std::time::{Duration, Instant};

const FG_DIM: u8 = 242;
const BG_BAR: u8 = 236;
const BG_SEL: u8 = 237;
const ACCENT: u8 = 180;
const C_OK: u8 = 114;
const C_BAD: u8 = 203;
const C_WARN: u8 = 179;
const C_MERGED: u8 = 176;
const C_LINK: u8 = 75;
const C_CODE: u8 = 180;
const NAME_COLORS: [u8; 12] = [167, 173, 179, 143, 107, 72, 74, 110, 104, 140, 175, 139];

/// Lists are re-fetched this often; unchanged ones answer 304.
const REFRESH: Duration = Duration::from_secs(60);

const TABS: [&str; 5] = ["review", "mine", "issues", "inbox", "repo"];
const REPO_TAB: usize = 4;
const INBOX_TAB: usize = 3;

pub const HELP: &str = "\
lists:
  j k ↑ ↓      select              gg G  first / last
  Enter l      open                1-5 Tab H L  switch tab
  /            filter (Enter keeps it, Esc clears)
  r            refresh             o  open in browser
  y            copy URL (OSC 52)   m  mark notification read
  C-k          pick a repo         q  quit   C-z suspend
tabs: 1 review requested, 2 my PRs, 3 my issues, 4 inbox, 5 repo
item:
  j k C-d C-u  scroll              g G  top / bottom
  d            toggle the diff     ] [  next / prev file in diff
  c            comment             a  approve
  X            request changes     x  close / reopen
  M            merge (m merge, s squash, r rebase)
  o y r        browser, copy URL, refresh
  q Esc h      back
mouse:
  click        tab or row          double-click  open
  wheel        scroll              shift+drag    select text (terminal)
compose:
  Enter        send                Alt-Enter newline   Esc  cancel";

fn name_color(s: &str) -> u8 {
    NAME_COLORS[s.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % NAME_COLORS.len()]
}

/// Seconds since the epoch for `2026-10-06T11:25:42Z`.
fn epoch(iso: &str) -> Option<i64> {
    let n = |a: usize, b: usize| iso.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?)
}

fn age_at(iso: &str, now: i64) -> String {
    let Some(t) = epoch(iso) else { return String::new() };
    let s = (now - t).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        86400..=1209599 => format!("{}d", s / 86400),
        1209600..=5183999 => format!("{}w", s / 604800),
        _ => format!("{}mo", s / 2592000),
    }
}

fn age(iso: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    age_at(iso, now)
}

#[derive(Clone, Default)]
struct Item {
    repo: String,
    num: u64,
    pr: bool,
    title: String,
    author: String,
    draft: bool,
    updated: String,
    comments: u64,
    /// Notification thread id, and why it's in the inbox.
    notif: Option<String>,
    unread: bool,
    reason: String,
    url: String,
    label: String,
}

impl Item {
    fn finish(mut self) -> Item {
        self.label = format!("{} #{} {} {} {}", self.repo, self.num, self.title, self.author, self.reason);
        self
    }

    fn from_search(v: &Value) -> Item {
        let repo = v.get("repository_url").str().split_once("/repos/").map_or("", |x| x.1).to_string();
        Item {
            repo,
            num: v.get("number").num() as u64,
            pr: !v.get("pull_request").is_null(),
            title: v.get("title").str().into(),
            author: v.path("user.login").str().into(),
            draft: v.get("draft").bool(),
            updated: v.get("updated_at").str().into(),
            comments: v.get("comments").num() as u64,
            url: v.get("html_url").str().into(),
            ..Item::default()
        }
        .finish()
    }

    fn from_notif(v: &Value) -> Item {
        let s = v.get("subject");
        let mut seg = s.get("url").str().rsplit('/');
        let num = seg.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let pr = seg.next() == Some("pulls");
        let issue = pr || s.get("type").str() == "Issue";
        let home = v.path("repository.html_url").str();
        let url = match (num, pr) {
            (0, _) => home.to_string(),
            (n, true) => format!("{}/pull/{}", home, n),
            (n, false) => format!("{}/issues/{}", home, n),
        };
        Item {
            repo: v.path("repository.full_name").str().into(),
            num: if issue { num } else { 0 },
            pr,
            title: s.get("title").str().into(),
            updated: v.get("updated_at").str().into(),
            notif: v.get("id").opt_str().map(String::from),
            unread: v.get("unread").bool(),
            reason: v.get("reason").str().replace('_', " "),
            url,
            ..Item::default()
        }
        .finish()
    }
}

#[derive(Default)]
struct List {
    items: Vec<Item>,
    etag: Option<String>,
    loaded: bool,
    busy: bool,
    sel: usize,
    top: usize,
}

struct Detail {
    gen: u64,
    item: Item,
    issue: Value,
    pull: Value,
    comments: Vec<Value>,
    reviews: Vec<Value>,
    checks: Value,
    diff: Option<String>,
    show_diff: bool,
    scroll: usize,
    ver: u64,
    err: Option<String>,
}

struct DL {
    ind: usize,
    text: String,
    st: Style,
}

fn dl(ind: usize, text: impl Into<String>, st: Style) -> DL {
    DL { ind, text: text.into(), st }
}

impl Detail {
    fn is_open(&self) -> bool {
        match self.issue.get("state").opt_str() {
            Some(s) => s == "open",
            None => true,
        }
    }

    fn merged(&self) -> bool {
        self.pull.get("merged").bool()
    }

    fn doc(&self) -> Vec<DL> {
        let plain = Style::default();
        let dim = Style::new(FG_DIM, 0, 0);
        let mut d = Vec::new();
        if self.show_diff {
            return match &self.diff {
                None => vec![dl(0, "loading diff…", Style::new(FG_DIM, 0, ITALIC))],
                Some(t) => t
                    .lines()
                    .map(|l| {
                        let st = if l.starts_with("diff --git") {
                            Style::new(ACCENT, 0, BOLD)
                        } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("index ") || l.starts_with("new file") || l.starts_with("deleted file") || l.starts_with("similarity") || l.starts_with("rename ") {
                            Style::new(FG_DIM, 0, BOLD)
                        } else if l.starts_with("@@") {
                            Style::fg(73)
                        } else if l.starts_with('+') {
                            Style::fg(C_OK)
                        } else if l.starts_with('-') {
                            Style::fg(C_BAD)
                        } else {
                            plain
                        };
                        dl(0, l, st)
                    })
                    .collect(),
            };
        }
        let it = &self.item;
        let title = self.issue.get("title").opt_str().unwrap_or(&it.title);
        d.push(dl(0, format!("{} #{}  {}", it.repo, it.num, title), Style::new(255, 0, BOLD)));
        let (state, sc) = if self.merged() {
            ("merged", C_MERGED)
        } else if self.is_open() {
            (if self.pull.get("draft").bool() || it.draft { "draft" } else { "open" }, C_OK)
        } else {
            ("closed", C_BAD)
        };
        let author = self.issue.path("user.login").opt_str().unwrap_or(&it.author);
        let mut meta = format!(" · {} · {} ago", author, age(self.issue.get("created_at").str()));
        if it.pr && !self.pull.is_null() {
            meta += &format!(" · {} ← {} · +{} −{} · {} files", self.pull.path("base.ref").str(), self.pull.path("head.ref").str(), self.pull.get("additions").num(), self.pull.get("deletions").num(), self.pull.get("changed_files").num());
        }
        d.push(dl(0, format!("{}{}", state, meta), Style::fg(sc)));
        let names = |v: &Value, key: &str| v.arr().iter().map(|x| x.get(key).str().to_string()).collect::<Vec<_>>().join(", ");
        let labels = names(self.issue.get("labels"), "name");
        if !labels.is_empty() {
            d.push(dl(0, format!("labels: {}", labels), dim));
        }
        let assignees = names(self.issue.get("assignees"), "login");
        if !assignees.is_empty() {
            d.push(dl(0, format!("assignees: {}", assignees), dim));
        }
        if it.pr {
            let asked = names(self.pull.get("requested_reviewers"), "login");
            let mut by: Vec<(String, String)> = Vec::new();
            for r in &self.reviews {
                let (u, s) = (r.path("user.login").str(), r.get("state").str());
                if s == "COMMENTED" || s == "PENDING" {
                    continue;
                }
                by.retain(|(x, _)| x != u);
                by.push((u.into(), s.into()));
            }
            let mut parts: Vec<String> = by.iter().map(|(u, s)| format!("{} {}", match s.as_str() { "APPROVED" => "✓", "CHANGES_REQUESTED" => "✗", _ => "–" }, u)).collect();
            if !asked.is_empty() {
                parts.push(format!("… {}", asked));
            }
            if !parts.is_empty() {
                let bad = by.iter().any(|(_, s)| s == "CHANGES_REQUESTED");
                d.push(dl(0, format!("reviews: {}", parts.join("  ")), Style::fg(if bad { C_BAD } else { 250 })));
            }
            let runs = self.checks.get("check_runs").arr();
            if !runs.is_empty() {
                let bad: Vec<&str> = runs.iter().filter(|r| matches!(r.get("conclusion").str(), "failure" | "timed_out" | "cancelled" | "action_required")).map(|r| r.get("name").str()).collect();
                let pending = runs.iter().filter(|r| r.get("status").str() != "completed").count();
                let ok = runs.len() - bad.len() - pending;
                let mut s = format!("checks: {} passed", ok);
                if pending > 0 {
                    s += &format!(", {} running", pending);
                }
                if !bad.is_empty() {
                    s += &format!(", {} failed ({})", bad.len(), bad.join(", "));
                }
                d.push(dl(0, s, Style::fg(if !bad.is_empty() { C_BAD } else if pending > 0 { C_WARN } else { C_OK })));
            }
            if let Some(ms) = self.pull.get("mergeable_state").opt_str() {
                if self.is_open() {
                    d.push(dl(0, format!("mergeable: {}", ms), Style::fg(if ms == "clean" { C_OK } else { C_WARN })));
                }
            }
        }
        if let Some(e) = &self.err {
            d.push(dl(0, e.clone(), Style::fg(C_BAD)));
        }
        d.push(dl(0, "", plain));
        let body = self.issue.get("body").str();
        if self.issue.is_null() {
            d.push(dl(0, "loading…", Style::new(FG_DIM, 0, ITALIC)));
        } else if body.trim().is_empty() {
            d.push(dl(0, "(no description)", Style::new(FG_DIM, 0, ITALIC)));
        } else {
            markdown(body, 0, &mut d);
        }
        let mut tl: Vec<(&str, &str, String, &Value)> = Vec::new();
        for c in &self.comments {
            tl.push((c.get("created_at").str(), c.path("user.login").str(), String::new(), c));
        }
        for r in &self.reviews {
            let s = r.get("state").str();
            if s == "PENDING" || (s == "COMMENTED" && r.get("body").str().trim().is_empty()) {
                continue;
            }
            let tag = match s {
                "APPROVED" => "approved",
                "CHANGES_REQUESTED" => "requested changes",
                "DISMISSED" => "review dismissed",
                _ => "reviewed",
            };
            tl.push((r.get("submitted_at").str(), r.path("user.login").str(), tag.into(), r));
        }
        tl.sort_by(|a, b| a.0.cmp(b.0));
        for (at, who, tag, v) in tl {
            d.push(dl(0, "", plain));
            let x = format!("{}{}  {} ago", who, if tag.is_empty() { String::new() } else { format!(" {}", tag) }, age(at));
            d.push(dl(0, x, Style::new(name_color(who), 0, BOLD)));
            let b = v.get("body").str();
            if !b.trim().is_empty() {
                markdown(b, 2, &mut d);
            }
        }
        d
    }
}

/// Light markdown styling: fences, headings, quotes. Everything else is text.
fn markdown(s: &str, ind: usize, d: &mut Vec<DL>) {
    let mut fence = false;
    for l in s.lines() {
        let l = l.trim_end_matches('\r');
        if l.trim_start().starts_with("```") {
            fence = !fence;
            d.push(dl(ind, l, Style::fg(FG_DIM)));
        } else if fence {
            d.push(dl(ind, l, Style::fg(C_CODE)));
        } else if l.starts_with('#') {
            d.push(dl(ind, l.trim_start_matches('#').trim_start(), Style::new(ACCENT, 0, BOLD)));
        } else if l.starts_with('>') {
            d.push(dl(ind, l, Style::new(FG_DIM, 0, ITALIC)));
        } else {
            d.push(dl(ind, l, Style::default()));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Purpose {
    Comment,
    Approve,
    Changes,
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    State,
    Merge,
}

enum Mode {
    Normal,
    Filter(LineEdit),
    Compose(Purpose, LineEdit),
    Confirm(Confirm),
    Pick(Picker<String>),
    Help,
}

#[derive(Default)]
struct Geo {
    tabs: Vec<(usize, usize, usize)>,
    y0: usize,
    rows: usize,
    top: usize,
}

pub struct App {
    pub screen: Screen,
    net: Net,
    pub quit: bool,
    pub suspend: bool,
    me: String,
    repo: Option<String>,
    tab: usize,
    lists: [List; 5],
    filter: String,
    detail: Option<Detail>,
    gen: u64,
    mode: Mode,
    msg: String,
    err: bool,
    pending: usize,
    repos: Vec<String>,
    repos_loaded: bool,
    want_pick: bool,
    last_refresh: Instant,
    geo: Geo,
    last_click: Option<(Instant, usize)>,
    pending_g: bool,
}

impl App {
    pub fn new(w: usize, h: usize, net: Net, repo: Option<String>, explicit: bool) -> App {
        let mut app = App {
            screen: Screen::new(w, h),
            net,
            quit: false,
            suspend: false,
            me: String::new(),
            tab: if explicit { REPO_TAB } else { 0 },
            repo,
            lists: Default::default(),
            filter: String::new(),
            detail: None,
            gen: 0,
            mode: Mode::Normal,
            msg: "connecting…".into(),
            err: false,
            pending: 0,
            repos: Vec::new(),
            repos_loaded: false,
            want_pick: false,
            last_refresh: Instant::now(),
            geo: Geo::default(),
            last_click: None,
            pending_g: false,
        };
        app.call(Tag::User, Req::get("/user"));
        app.refresh_all(true);
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

    fn call(&mut self, tag: Tag, req: Req) {
        self.pending += 1;
        self.net.call(tag, req);
    }

    // ---- lists ----

    fn list_req(&self, i: usize) -> Option<Req> {
        let search = |q: &str| Req::get(format!("/search/issues?q={}&per_page=50&sort=updated&order=desc", mhttp::urlencode(q)));
        Some(match i {
            0 => search("is:open is:pr review-requested:@me archived:false"),
            1 => search("is:open is:pr author:@me archived:false"),
            2 => search("is:open is:issue assignee:@me archived:false"),
            INBOX_TAB => Req::get("/notifications?per_page=50"),
            _ => search(&format!("repo:{} is:open archived:false", self.repo.as_ref()?)),
        })
    }

    fn refresh_list(&mut self, i: usize, force: bool) {
        if self.lists[i].busy {
            return;
        }
        let Some(mut req) = self.list_req(i) else { return };
        if !force {
            req.etag = self.lists[i].etag.clone();
        }
        self.lists[i].busy = true;
        self.call(Tag::List(i), req);
    }

    fn refresh_all(&mut self, force: bool) {
        self.last_refresh = Instant::now();
        for i in 0..TABS.len() {
            self.refresh_list(i, force);
        }
    }

    fn on_list(&mut self, i: usize, res: Result<api::Resp, String>) {
        self.lists[i].busy = false;
        let r = match res {
            Ok(r) => r,
            Err(e) => return self.error(format!("{}: {}", TABS[i], e)),
        };
        let l = &mut self.lists[i];
        l.loaded = true;
        if r.status == 304 {
            return;
        }
        l.etag = r.etag;
        let arr = if i == INBOX_TAB { r.body.arr() } else { r.body.get("items").arr() };
        let keep = l.items.get(l.sel).map(|it| (it.repo.clone(), it.num, it.notif.clone()));
        l.items = arr.iter().map(|v| if i == INBOX_TAB { Item::from_notif(v) } else { Item::from_search(v) }).collect();
        if let Some(k) = keep {
            if let Some(p) = l.items.iter().position(|it| (it.repo.clone(), it.num, it.notif.clone()) == k) {
                l.sel = p;
            }
        }
        l.sel = l.sel.min(l.items.len().saturating_sub(1));
    }

    fn vis(&self) -> Vec<usize> {
        let l = &self.lists[self.tab];
        if self.filter.is_empty() {
            return (0..l.items.len()).collect();
        }
        fuzzy::filter(&self.filter, l.items.iter().map(|it| it.label.as_str()))
    }

    fn cur_item(&self) -> Option<Item> {
        if let Some(d) = &self.detail {
            return Some(d.item.clone());
        }
        let l = &self.lists[self.tab];
        self.vis().get(l.sel).map(|&i| l.items[i].clone())
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.vis().len() as isize;
        let l = &mut self.lists[self.tab];
        l.sel = (l.sel as isize + d).clamp(0, (n - 1).max(0)) as usize;
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t % TABS.len();
        self.filter.clear();
        if self.tab == REPO_TAB && self.repo.is_none() {
            self.info("no repo; pick one with C-k");
        }
    }

    fn set_repo(&mut self, r: String) {
        self.repo = Some(r);
        self.lists[REPO_TAB] = List::default();
        self.set_tab(REPO_TAB);
        self.refresh_list(REPO_TAB, true);
    }

    // ---- detail ----

    fn open_item(&mut self, it: Item) {
        if it.num == 0 {
            return self.open_url(&it.url);
        }
        self.gen += 1;
        let (g, r, n) = (self.gen, it.repo.clone(), it.num);
        self.call(Tag::Issue(g), Req::get(format!("/repos/{}/issues/{}", r, n)));
        self.call(Tag::Comments(g), Req::get(format!("/repos/{}/issues/{}/comments?per_page=100", r, n)));
        if it.pr {
            self.call(Tag::Pull(g), Req::get(format!("/repos/{}/pulls/{}", r, n)));
            self.call(Tag::Reviews(g), Req::get(format!("/repos/{}/pulls/{}/reviews?per_page=100", r, n)));
        }
        if let (Some(id), true) = (&it.notif, it.unread) {
            self.call(Tag::Act("marked read"), Req::send("PATCH", format!("/notifications/threads/{}", id), String::new()));
            for l in self.lists.iter_mut() {
                for x in l.items.iter_mut().filter(|x| x.notif == it.notif) {
                    x.unread = false;
                }
            }
        }
        self.detail = Some(Detail { gen: g, item: it, issue: Value::Null, pull: Value::Null, comments: Vec::new(), reviews: Vec::new(), checks: Value::Null, diff: None, show_diff: false, scroll: 0, ver: 0, err: None });
        self.info("");
    }

    fn reload_detail(&mut self) {
        let Some(d) = self.detail.take() else { return };
        let (show, scroll) = (d.show_diff, d.scroll);
        self.open_item(d.item);
        if let Some(d) = &mut self.detail {
            d.scroll = scroll;
            if show {
                d.show_diff = true;
                self.fetch_diff();
            }
        }
    }

    fn fetch_diff(&mut self) {
        let Some(d) = &self.detail else { return };
        let req = Req { accept: api::DIFF, ..Req::get(format!("/repos/{}/pulls/{}", d.item.repo, d.item.num)) };
        let g = d.gen;
        self.call(Tag::Diff(g), req);
    }

    fn on_detail(&mut self, tag: Tag, res: Result<api::Resp, String>) {
        let g = match tag {
            Tag::Issue(g) | Tag::Comments(g) | Tag::Pull(g) | Tag::Reviews(g) | Tag::Checks(g) | Tag::Diff(g) => g,
            _ => return,
        };
        let Some(d) = self.detail.as_mut().filter(|d| d.gen == g) else { return };
        d.ver += 1;
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                d.err = Some(format!("{}", e));
                return;
            }
        };
        let list = |v: Value| match v {
            Value::Arr(a) => a,
            _ => Vec::new(),
        };
        match tag {
            Tag::Issue(_) => d.issue = r.body,
            Tag::Comments(_) => d.comments = list(r.body),
            Tag::Reviews(_) => d.reviews = list(r.body),
            Tag::Diff(_) => d.diff = Some(r.text),
            Tag::Checks(_) => d.checks = r.body,
            Tag::Pull(_) => {
                let sha = r.body.path("head.sha").str().to_string();
                d.pull = r.body;
                if !sha.is_empty() {
                    let req = Req::get(format!("/repos/{}/commits/{}/check-runs?per_page=100", d.item.repo, sha));
                    self.call(Tag::Checks(g), req);
                }
            }
            _ => {}
        }
    }

    fn close_detail(&mut self) {
        self.detail = None;
        self.info("");
        self.refresh_list(self.tab, false);
    }

    // ---- actions ----

    fn act(&mut self, label: &'static str, req: Req) {
        self.info(format!("{}…", label));
        self.call(Tag::Act(label), req);
    }

    fn compose(&mut self, p: Purpose) {
        match &self.detail {
            Some(d) if d.item.pr || p == Purpose::Comment => self.mode = Mode::Compose(p, LineEdit::new()),
            _ => self.error("only pull requests can be reviewed"),
        }
    }

    fn send_compose(&mut self, p: Purpose, text: &str) {
        let Some(d) = &self.detail else { return };
        let (r, n) = (d.item.repo.clone(), d.item.num);
        let text = text.trim();
        if text.is_empty() && p != Purpose::Approve {
            return self.error("empty message");
        }
        let b = json::quote(text);
        match p {
            Purpose::Comment => self.act("commented", Req::send("POST", format!("/repos/{}/issues/{}/comments", r, n), format!("{{\"body\":{}}}", b))),
            Purpose::Approve => self.act("approved", Req::send("POST", format!("/repos/{}/pulls/{}/reviews", r, n), format!("{{\"event\":\"APPROVE\",\"body\":{}}}", b))),
            Purpose::Changes => self.act("requested changes", Req::send("POST", format!("/repos/{}/pulls/{}/reviews", r, n), format!("{{\"event\":\"REQUEST_CHANGES\",\"body\":{}}}", b))),
        }
    }

    fn confirm_key(&mut self, c: Confirm, k: Key) {
        let Some(d) = &self.detail else { return };
        let (r, n, open) = (d.item.repo.clone(), d.item.num, d.is_open());
        match (c, k) {
            (Confirm::State, Key::Char('y')) => {
                let (label, state) = if open { ("closed", "closed") } else { ("reopened", "open") };
                self.act(label, Req::send("PATCH", format!("/repos/{}/issues/{}", r, n), format!("{{\"state\":\"{}\"}}", state)));
            }
            (Confirm::Merge, Key::Char(m @ ('m' | 's' | 'r'))) => {
                let method = match m {
                    'm' => "merge",
                    's' => "squash",
                    _ => "rebase",
                };
                self.act("merged", Req::send("PUT", format!("/repos/{}/pulls/{}/merge", r, n), format!("{{\"merge_method\":\"{}\"}}", method)));
            }
            _ => self.info(""),
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

    fn mark_read(&mut self) {
        let Some(it) = self.cur_item() else { return };
        let Some(id) = it.notif else { return };
        self.call(Tag::Act("marked read"), Req::send("PATCH", format!("/notifications/threads/{}", id), String::new()));
        let l = &mut self.lists[INBOX_TAB];
        l.items.retain(|x| x.notif.as_deref() != Some(id.as_str()));
        l.sel = l.sel.min(l.items.len().saturating_sub(1));
    }

    pub fn tick(&mut self) -> i32 {
        if !self.me.is_empty() && self.last_refresh.elapsed() >= REFRESH {
            self.refresh_all(false);
        }
        1000
    }

    pub fn on_reply(&mut self, (tag, res): Reply) {
        self.pending = self.pending.saturating_sub(1);
        match tag {
            Tag::User => match res {
                Ok(r) => {
                    self.me = r.body.get("login").str().to_string();
                    self.info(format!("signed in as {}", self.me));
                }
                Err(e) => self.error(format!("auth: {}", e)),
            },
            Tag::List(i) => self.on_list(i, res),
            Tag::Repos => match res {
                Ok(r) => {
                    self.repos = r.body.arr().iter().map(|v| v.get("full_name").str().to_string()).collect();
                    self.repos_loaded = true;
                    if self.want_pick {
                        self.want_pick = false;
                        self.mode = Mode::Pick(Picker::new("repo", self.repos.clone()));
                    }
                }
                Err(e) => {
                    self.want_pick = false;
                    self.error(format!("repos: {}", e));
                }
            },
            Tag::Act(label) => match res {
                Ok(_) => {
                    if label != "marked read" {
                        self.info(label);
                        if self.detail.is_some() {
                            self.reload_detail();
                        }
                        self.refresh_all(true);
                    }
                }
                Err(e) => self.error(format!("{}: {}", label, e)),
            },
            t => self.on_detail(t, res),
        }
    }

    // ---- keys ----

    fn pick_repo(&mut self) {
        if self.repos_loaded {
            self.mode = Mode::Pick(Picker::new("repo", self.repos.clone()));
        } else {
            self.want_pick = true;
            self.info("loading repos…");
            self.call(Tag::Repos, Req::get("/user/repos?sort=pushed&per_page=100"));
        }
    }

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
        if k == Key::Ctrl('k') && matches!(self.mode, Mode::Normal) {
            return self.pick_repo();
        }
        if let Key::Mouse(m) = k {
            match &self.mode {
                Mode::Normal => return self.mouse(m),
                Mode::Help if matches!(m.kind, MouseKind::Press(_)) => {
                    self.mode = Mode::Normal;
                    return;
                }
                Mode::Pick(_) => {}
                _ => return,
            }
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.normal_key(k),
            Mode::Filter(mut e) => match k {
                Key::Esc | Key::Ctrl('c') => {
                    self.filter.clear();
                    self.lists[self.tab].sel = 0;
                }
                Key::Enter => {}
                k => {
                    e.key(&k);
                    self.filter = e.text.clone();
                    self.lists[self.tab].sel = 0;
                    self.mode = Mode::Filter(e);
                }
            },
            Mode::Compose(p, mut e) => match k {
                Key::Esc | Key::Ctrl('c') => {}
                Key::Enter => self.send_compose(p, &e.take()),
                k => {
                    e.key(&k);
                    self.mode = Mode::Compose(p, e);
                }
            },
            Mode::Confirm(c) => self.confirm_key(c, k),
            Mode::Pick(mut p) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Pick(p),
                Pick::Cancel => {}
                Pick::Accept => {
                    let r = p.selected().cloned().or_else(|| p.query.contains('/').then(|| p.query.trim().to_string()));
                    if let Some(r) = r {
                        self.detail = None;
                        self.set_repo(r);
                    }
                }
            },
            Mode::Help => {}
        }
    }

    fn normal_key(&mut self, k: Key) {
        let g = std::mem::take(&mut self.pending_g);
        if self.detail.is_some() {
            return self.detail_key(k, g);
        }
        let page = (self.geo.rows / 2).max(1) as isize;
        match k {
            Key::Char('q') => self.quit = true,
            Key::Char('?') => self.mode = Mode::Help,
            Key::Char('j') | Key::Down | Key::Ctrl('n') => self.move_sel(1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => self.move_sel(-1),
            Key::Ctrl('d') | Key::PageDown => self.move_sel(page),
            Key::Ctrl('u') | Key::PageUp => self.move_sel(-page),
            Key::Char('G') | Key::End => self.move_sel(isize::MAX / 2),
            Key::Home => self.move_sel(isize::MIN / 2),
            Key::Char('g') if g => self.move_sel(isize::MIN / 2),
            Key::Char('g') => self.pending_g = true,
            Key::Char(c @ '1'..='5') => self.set_tab(c as usize - '1' as usize),
            Key::Tab | Key::Char('L') => self.set_tab(self.tab + 1),
            Key::BackTab | Key::Char('H') => self.set_tab(self.tab + TABS.len() - 1),
            Key::Char('/') => self.mode = Mode::Filter(LineEdit::new()),
            Key::Esc if !self.filter.is_empty() => self.filter.clear(),
            Key::Char('r') => self.refresh_all(true),
            Key::Enter | Key::Char('l') => {
                if let Some(it) = self.cur_item() {
                    self.open_item(it);
                }
            }
            Key::Char('o') => {
                if let Some(it) = self.cur_item() {
                    self.open_url(&it.url);
                }
            }
            Key::Char('y') => self.yank(),
            Key::Char('m') if self.tab == INBOX_TAB => self.mark_read(),
            Key::Char('R') => self.pick_repo(),
            _ => {}
        }
    }

    fn yank(&mut self) {
        if let Some(it) = self.cur_item() {
            self.screen.osc52(&it.url);
            self.info(format!("copied {}", it.url));
        }
    }

    fn detail_key(&mut self, k: Key, g: bool) {
        let page = (self.screen.h.saturating_sub(2) / 2).max(1) as isize;
        let Some(d) = &mut self.detail else { return };
        let show_diff = d.show_diff;
        let scroll = |d: &mut Detail, by: isize| d.scroll = (d.scroll as isize + by).max(0) as usize;
        match k {
            Key::Char('q') | Key::Esc | Key::Char('h') | Key::Left => {
                if show_diff {
                    d.show_diff = false;
                    d.scroll = 0;
                } else {
                    self.close_detail();
                }
            }
            Key::Char('?') => self.mode = Mode::Help,
            Key::Char('j') | Key::Down | Key::Ctrl('n') | Key::Enter => scroll(d, 1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => scroll(d, -1),
            Key::Ctrl('d') | Key::PageDown | Key::Char(' ') | Key::Ctrl('f') => scroll(d, page),
            Key::Ctrl('u') | Key::PageUp | Key::Ctrl('b') => scroll(d, -page),
            Key::Char('G') | Key::End => d.scroll = usize::MAX / 2,
            Key::Home => d.scroll = 0,
            Key::Char('g') if g => d.scroll = 0,
            Key::Char('g') => self.pending_g = true,
            Key::Char('d') if d.item.pr => {
                d.show_diff = !show_diff;
                d.scroll = 0;
                if d.show_diff && d.diff.is_none() {
                    self.fetch_diff();
                }
            }
            Key::Char(c @ (']' | '[')) if show_diff => {
                let starts: Vec<usize> = self.diff_starts();
                let Some(d) = &mut self.detail else { return };
                let cur = d.scroll;
                let t = if c == ']' { starts.iter().copied().find(|&s| s > cur) } else { starts.iter().copied().rev().find(|&s| s < cur) };
                if let Some(t) = t {
                    d.scroll = t;
                }
            }
            Key::Char('c') => self.compose(Purpose::Comment),
            Key::Char('a') => self.compose(Purpose::Approve),
            Key::Char('X') => self.compose(Purpose::Changes),
            Key::Char('x') => self.mode = Mode::Confirm(Confirm::State),
            Key::Char('M') if d.item.pr && d.is_open() => self.mode = Mode::Confirm(Confirm::Merge),
            Key::Char('r') => self.reload_detail(),
            Key::Char('o') => {
                let u = d.item.url.clone();
                self.open_url(&u);
            }
            Key::Char('y') => self.yank(),
            _ => {}
        }
    }

    /// Wrapped-row index of each `diff --git` header at the last drawn width.
    fn diff_starts(&self) -> Vec<usize> {
        let Some(d) = &self.detail else { return Vec::new() };
        let w = self.screen.w;
        let mut row = 0;
        let mut v = Vec::new();
        for l in d.doc() {
            if l.text.starts_with("diff --git") {
                v.push(row);
            }
            row += wrap(&l.text, w.saturating_sub(l.ind).max(1)).len();
        }
        v
    }

    fn mouse(&mut self, m: Mouse) {
        if let Some(d) = &mut self.detail {
            match m.kind {
                MouseKind::WheelUp => d.scroll = d.scroll.saturating_sub(3),
                MouseKind::WheelDown => d.scroll += 3,
                _ => {}
            }
            return;
        }
        match m.kind {
            MouseKind::WheelUp => self.move_sel(-3),
            MouseKind::WheelDown => self.move_sel(3),
            MouseKind::Press(0) if m.y == 0 => {
                if let Some(&(_, _, t)) = self.geo.tabs.iter().find(|&&(a, b, _)| (a..b).contains(&m.x)) {
                    self.set_tab(t);
                }
            }
            MouseKind::Press(0) if m.y >= self.geo.y0 && m.y < self.geo.y0 + self.geo.rows => {
                let i = self.geo.top + m.y - self.geo.y0;
                if i < self.vis().len() {
                    self.lists[self.tab].sel = i;
                    let dbl = self.last_click.is_some_and(|(t, j)| j == i && t.elapsed() < Duration::from_millis(400));
                    self.last_click = Some((Instant::now(), i));
                    if dbl {
                        if let Some(it) = self.cur_item() {
                            self.open_item(it);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- drawing ----

    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if w < 20 || h < 5 {
            self.screen.puts(0, 0, "too small", Style::default(), w);
            self.screen.flush(None);
            return;
        }
        let y_status = h - 1;
        let mut cursor = None;
        let mut bottom = y_status; // first row not available to the body

        if let Mode::Compose(p, e) = &self.mode {
            let prompt = match p {
                Purpose::Comment => "comment› ",
                Purpose::Approve => "approve› ",
                Purpose::Changes => "changes› ",
            };
            let pw = str_width(prompt);
            let rows = e.height(w.saturating_sub(pw + 1), 0).clamp(1, (h / 3).max(1));
            bottom = y_status - rows - 1;
            for x in 0..w {
                self.screen.put(x, bottom, '─', Style::fg(238));
            }
            self.screen.puts(0, bottom + 1, prompt, Style::new(ACCENT, 0, BOLD), w);
            let (cx, cy) = e.draw(&mut self.screen, pw, bottom + 1, w.saturating_sub(pw + 1), rows, Style::new(255, 0, 0));
            cursor = Some((cx, cy, true));
        }

        if self.detail.is_some() {
            self.render_detail(bottom);
        } else {
            self.render_list(bottom);
        }

        // status line
        let (tag, tag_bg) = match &self.mode {
            Mode::Filter(_) => (" FILTER ", 108),
            Mode::Compose(..) => (" COMPOSE ", 108),
            Mode::Confirm(_) => (" CONFIRM ", 203),
            Mode::Pick(_) => (" PICK ", 180),
            _ => match &self.detail {
                Some(d) if d.show_diff => (" DIFF ", 73),
                Some(_) => (" ITEM ", 110),
                None => (" LIST ", 110),
            },
        };
        self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
        let x = self.screen.puts(0, y_status, tag, Style::new(16, tag_bg, BOLD), w) + 1;
        match &self.mode {
            Mode::Filter(e) => {
                let x2 = self.screen.puts(x, y_status, "/", Style::new(ACCENT, BG_BAR, 0), w);
                let (cx, cy) = e.draw(&mut self.screen, x2, y_status, w.saturating_sub(x2), 1, Style::new(255, BG_BAR, 0));
                cursor = Some((cx, cy, true));
            }
            Mode::Confirm(c) => {
                let open = self.detail.as_ref().is_some_and(|d| d.is_open());
                let s = match c {
                    Confirm::State if open => "close this? (y/n)",
                    Confirm::State => "reopen this? (y/n)",
                    Confirm::Merge => "merge: (m)erge commit, (s)quash, (r)ebase, any other key cancels",
                };
                self.screen.puts(x, y_status, s, Style::new(C_BAD, BG_BAR, BOLD), w);
            }
            _ => {
                let st = Style::new(if self.err { C_BAD } else { 250 }, BG_BAR, 0);
                let s = if !self.filter.is_empty() && self.detail.is_none() { format!("filter: {}", self.filter) } else { self.msg.lines().next().unwrap_or("").to_string() };
                self.screen.puts(x, y_status, &s, st, w);
            }
        }
        let right = format!("{}{}{}? help ", if self.pending > 0 { "… " } else { "" }, self.me, if self.me.is_empty() { "" } else { "  " });
        self.screen.puts(w.saturating_sub(str_width(&right)), y_status, &right, Style::new(FG_DIM, BG_BAR, 0), w);

        match &mut self.mode {
            Mode::Pick(p) => {
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
                    let st = if l.ends_with(':') || l.starts_with("tabs:") { Style::new(ACCENT, BG_BAR, BOLD) } else { Style::new(252, BG_BAR, 0) };
                    self.screen.puts(bx + 2, by + i, l, st, bx + bw);
                }
                cursor = None;
            }
            _ => {}
        }
        self.screen.flush(cursor);
    }

    fn render_list(&mut self, bottom: usize) {
        let w = self.screen.w;
        // tab bar
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        self.geo.tabs.clear();
        let mut x = 0;
        for (i, name) in TABS.iter().enumerate() {
            let name = match (i, &self.repo) {
                (REPO_TAB, Some(r)) => r.as_str(),
                _ => name,
            };
            let n = self.lists[i].items.iter().filter(|it| i != INBOX_TAB || it.unread).count();
            let s = format!(" {} {}{} ", i + 1, name, if n > 0 { format!(" {}", n) } else { String::new() });
            let cur = i == self.tab;
            let x1 = self.screen.puts(x, 0, &s, if cur { Style::new(16, ACCENT, BOLD) } else { Style::new(250, BG_BAR, 0) }, w);
            self.geo.tabs.push((x, x1, i));
            x = x1 + 1;
        }

        let vis = self.vis();
        let (y0, rows) = (1, bottom.saturating_sub(1));
        let l = &mut self.lists[self.tab];
        l.sel = l.sel.min(vis.len().saturating_sub(1));
        if l.sel < l.top {
            l.top = l.sel;
        }
        if l.sel >= l.top + rows {
            l.top = l.sel + 1 - rows;
        }
        l.top = l.top.min(vis.len().saturating_sub(rows));
        (self.geo.y0, self.geo.rows, self.geo.top) = (y0, rows, l.top);
        if vis.is_empty() {
            let s = if !l.loaded && (self.tab != REPO_TAB || self.repo.is_some()) {
                "loading…"
            } else if self.tab == REPO_TAB && self.repo.is_none() {
                "no repo; press C-k to pick one"
            } else if !self.filter.is_empty() {
                "no matches"
            } else {
                "nothing here"
            };
            self.screen.puts(2, y0 + 1, s, Style::new(FG_DIM, 0, ITALIC), w);
            return;
        }
        let items = &l.items;
        let same_repo = self.tab == REPO_TAB;
        let repo_w = if same_repo { 0 } else { vis.iter().map(|&i| str_width(&items[i].repo)).max().unwrap_or(0).min(w / 4) + 1 };
        let num_w = vis.iter().map(|&i| items[i].num.to_string().len()).max().unwrap_or(1) + 1;
        for (k, &i) in vis.iter().enumerate().skip(l.top).take(rows) {
            let it = &items[i];
            let y = y0 + k - l.top;
            let sel = k == l.sel;
            let bg = if sel { BG_SEL } else { 0 };
            if sel {
                self.screen.fill(0, w, y, Style::new(0, bg, 0));
                self.screen.put(0, y, '▌', Style::new(ACCENT, bg, 0));
            }
            let dot = if it.unread { "•" } else { " " };
            self.screen.puts(1, y, dot, Style::new(C_LINK, bg, BOLD), w);
            let (kind, kc) = match (it.pr, it.draft) {
                (_, true) => ("PR", FG_DIM),
                (true, _) => ("PR", C_OK),
                (false, _) if it.num == 0 => ("··", FG_DIM),
                _ => ("IS", 140),
            };
            let mut x = self.screen.puts(3, y, kind, Style::new(kc, bg, BOLD), w) + 1;
            if repo_w > 0 {
                self.screen.puts(x, y, &it.repo, Style::new(FG_DIM, bg, 0), x + repo_w - 1);
                x += repo_w;
            }
            if it.num > 0 {
                let n = format!("#{}", it.num);
                self.screen.puts(x + num_w - str_width(&n), y, &n, Style::new(FG_DIM, bg, 0), w);
            }
            x += num_w + 1;
            let who = if it.reason.is_empty() { it.author.clone() } else { it.reason.clone() };
            let comments = if it.comments > 0 { format!(" {}c", it.comments) } else { String::new() };
            let right = format!("{}{}  {:>3} ", who, comments, age(&it.updated));
            let rw = str_width(&right).min(w / 2);
            let rx = w.saturating_sub(rw);
            self.screen.puts(x, y, &it.title, Style::new(if it.unread || !sel { 252 } else { 255 }, bg, if it.unread || sel { BOLD } else { 0 }), rx.saturating_sub(1));
            self.screen.puts(rx, y, &right, Style::new(if it.author.is_empty() { FG_DIM } else { name_color(&it.author) }, bg, 0), w);
        }
    }

    fn render_detail(&mut self, bottom: usize) {
        let w = self.screen.w;
        let Some(d) = &mut self.detail else { return };
        let doc = d.doc();
        let mut rows: Vec<(usize, &str, Style)> = Vec::new();
        for l in &doc {
            for (a, b) in wrap(&l.text, w.saturating_sub(l.ind + 1).max(1)) {
                rows.push((l.ind, &l.text[a..b], l.st));
            }
        }
        let view_h = bottom.saturating_sub(1);
        d.scroll = d.scroll.min(rows.len().saturating_sub(view_h));
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let title = format!(" {} #{}{} ", d.item.repo, d.item.num, if d.show_diff { " diff" } else { "" });
        self.screen.puts(0, 0, &title, Style::new(255, BG_BAR, BOLD), w);
        if rows.len() > view_h {
            let s = format!("{}% ", (d.scroll + view_h) * 100 / rows.len());
            self.screen.puts(w.saturating_sub(s.len()), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
        for (k, (ind, t, st)) in rows.iter().enumerate().skip(d.scroll).take(view_h) {
            let y = 1 + k - d.scroll;
            self.screen.puts(1 + ind, y, t, *st, w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        let t = epoch("2026-10-06T11:25:42Z").unwrap();
        assert_eq!(epoch("1970-01-02T00:00:00Z"), Some(86400));
        assert_eq!(epoch("2000-03-01T00:00:00Z"), Some(951868800));
        assert_eq!(age_at("2026-10-06T11:25:42Z", t + 90), "1m");
        assert_eq!(age_at("2026-10-06T11:25:42Z", t + 3 * 3600), "3h");
        assert_eq!(age_at("2026-10-06T11:25:42Z", t + 3 * 86400), "3d");
        assert_eq!(age_at("garbage", t), "");
    }

    #[test]
    fn notif_items() {
        let v = json::parse(r#"{"id":"9","unread":true,"reason":"review_requested","updated_at":"2026-01-01T00:00:00Z","subject":{"title":"Fix","url":"https://api.github.com/repos/a/b/pulls/12","type":"PullRequest"},"repository":{"full_name":"a/b","html_url":"https://github.com/a/b"}}"#).unwrap();
        let it = Item::from_notif(&v);
        assert!(it.pr && it.num == 12 && it.repo == "a/b" && it.reason == "review requested");
        assert_eq!(it.url, "https://github.com/a/b/pull/12");
        let v = json::parse(r#"{"id":"1","subject":{"title":"v1","url":"https://api.github.com/repos/a/b/releases/5","type":"Release"},"repository":{"full_name":"a/b","html_url":"https://github.com/a/b"}}"#).unwrap();
        assert_eq!(Item::from_notif(&v).num, 0);
    }
}
