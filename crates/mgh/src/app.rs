// mgh state, keys and drawing.

mod review;
mod code;
mod side;

use crate::api::{self, Net, Reply, Req, Tag};
use review::{Content, FileDiff, Pending, RComment, Rev};
use mtui::fuzzy;
use mtui::json::{self, Value};
use mtui::lineedit::LineEdit;
use mtui::picker::{Label, Pick, Picker};
use mtui::screen::{Screen, Style, BOLD, ITALIC, };
use mtui::term::{Key, Mouse, MouseKind};
use mtui::wrap::{str_width, wrap};
use std::collections::HashMap;
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
const NAME_COLORS: [u8; 12] = [167, 173, 179, 143, 107, 72, 74, 110, 104, 140, 175, 139];

/// Lists are re-fetched this often; unchanged ones answer 304.
const REFRESH: Duration = Duration::from_secs(60);

const TABS: [&str; 6] = ["review", "mine", "issues", "inbox", "repo", "code"];
const CODE_TAB: usize = 5;
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
code (tab 6):
  j k Enter    move, open folder/file   h  collapse / parent
  / C-p        find a file         Tab  tree <-> file   b  hide the tree
  J K          next / previous file (tree or file focus)
  file: j k C-d C-u g G scroll the cursor, h l sideways, o browser, y copy link to the line
  m            markdown files: rendered <-> source
  r reload the tree
tabs: 1 review requested, 2 my PRs, 3 my issues, 4 inbox, 5 repo, 6 code
item:
  j k C-d C-u  scroll              g G  top / bottom
  d            review the code     c  comment
  a            approve             X  request changes
  x            close / reopen
  M            merge (m merge, s squash, r rebase)
  o y r        browser, copy URL, refresh
  q Esc h      back
review (d):
  j k C-d C-u  move / page         g G  top / bottom   h l  scroll sideways
  ] [ J K      next / prev file    f  pick a file      m  mark viewed, next
  Tab          focus the file list b  hide / show it
  } {          next / prev hunk    n N  next / prev comment
  e            whole file          v  select lines     Esc  clear selection
  c            comment on the line(s), queued in a pending review
  r            reply to the thread x  drop a pending comment on the line
  S            submit: comment     a approve  X request changes (with pending)
  o            open on GitHub      q  back to the item
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
    rev: Rev,
    review: bool,
    scroll: usize,
    ver: u64,
    err: Option<String>,
}

struct DL {
    ind: usize,
    text: String,
    st: Style,
    /// `text` is markdown, rendered to the pane's width when drawn.
    md: bool,
}

fn dl(ind: usize, text: impl Into<String>, st: Style) -> DL {
    DL { ind, text: text.into(), st, md: false }
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
            d.push(DL { ind: 0, text: body.to_string(), st: Style::default(), md: true });
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
                d.push(DL { ind: 2, text: b.to_string(), st: Style::default(), md: true });
            }
        }
        d
    }
}

#[derive(Clone, PartialEq)]
enum Purpose {
    Comment,
    Approve,
    Changes,
    /// Submit the pending comments with a summary and no verdict.
    Review,
    /// A line comment, queued in the pending review.
    Inline { path: String, line: u32, start: Option<u32>, right: bool },
    Reply(u64),
}

struct FileItem(String, usize);
impl Label for FileItem {
    fn label(&self) -> &str {
        &self.0
    }
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
    Files(Picker<FileItem>),
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
    gallery: mimg::Gallery,
    side: side::SideGeo,
    side_map: Vec<Option<usize>>,
    drafts: HashMap<(String, u64), Vec<Pending>>,
    me: String,
    repo: Option<String>,
    tab: usize,
    lists: [List; 6],
    code: code::Browse,
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
    pub fn new(w: usize, h: usize, net: Net, repo: Option<String>, explicit: bool, images: bool) -> App {
        let mut app = App {
            screen: Screen::new(w, h),
            net,
            quit: false,
            suspend: false,
            gallery: mimg::Gallery::new(images),
            side: Default::default(),
            side_map: Vec::new(),
            drafts: HashMap::new(),
            me: String::new(),
            tab: if explicit { REPO_TAB } else { 0 },
            repo,
            lists: Default::default(),
            code: Default::default(),
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
            REPO_TAB => search(&format!("repo:{} is:open archived:false", self.repo.as_ref()?)),
            _ => return None,
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

    /// Mouse wheel: scroll the view and pull the selection inside it, so the
    /// next draw doesn't scroll back to the selection.
    fn scroll_list(&mut self, d: isize) {
        let n = self.vis().len();
        let rows = self.geo.rows.max(1);
        let l = &mut self.lists[self.tab];
        l.top = (l.top as isize + d).clamp(0, n.saturating_sub(rows) as isize) as usize;
        l.sel = l.sel.clamp(l.top, (l.top + rows).min(n).saturating_sub(1).max(l.top));
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t % TABS.len();
        self.filter.clear();
        if self.tab == CODE_TAB {
            self.ensure_tree(false);
        }
        if self.tab == REPO_TAB && self.repo.is_none() {
            self.info("no repo; pick one with C-k");
        }
    }

    fn set_repo(&mut self, r: String) {
        self.repo = Some(r);
        self.lists[REPO_TAB] = List::default();
        self.code = Default::default();
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
        self.detail = Some(Detail { gen: g, item: it, issue: Value::Null, pull: Value::Null, comments: Vec::new(), reviews: Vec::new(), checks: Value::Null, rev: Rev::default(), review: false, scroll: 0, ver: 0, err: None });
        self.info("");
    }

    fn reload_detail(&mut self) {
        let Some(mut d) = self.detail.take() else { return };
        let (review, scroll, rev) = (d.review, d.scroll, std::mem::take(&mut d.rev));
        let had_files = rev.files.is_some();
        self.open_item(d.item);
        if let Some(d) = &mut self.detail {
            d.scroll = scroll;
            d.review = review;
            d.rev = rev;
            d.rev.ver += 1;
        }
        if had_files {
            self.fetch_review();
        }
    }

    /// Request the PR's changed files and inline comments.
    fn fetch_review(&mut self) {
        let Some(d) = &self.detail else { return };
        let (g, r, n) = (d.gen, d.item.repo.clone(), d.item.num);
        self.call(Tag::Files(g, 1), Req::get(format!("/repos/{}/pulls/{}/files?per_page=100", r, n)));
        self.call(Tag::RComments(g), Req::get(format!("/repos/{}/pulls/{}/comments?per_page=100", r, n)));
    }

    fn on_detail(&mut self, tag: Tag, res: Result<api::Resp, String>) {
        let g = match tag {
            Tag::Issue(g) | Tag::Comments(g) | Tag::Pull(g) | Tag::Reviews(g) | Tag::Checks(g) | Tag::Files(g, _) | Tag::RComments(g) | Tag::Content(g, _) => g,
            _ => return,
        };
        let Some(d) = self.detail.as_mut().filter(|d| d.gen == g) else { return };
        d.ver += 1;
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                if let Tag::Content(_, path) = &tag {
                    d.rev.contents.insert(path.clone(), Content::Failed(e));
                    d.rev.ver += 1;
                } else {
                    d.err = Some(e);
                }
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
            Tag::Files(_, page) => {
                let files = d.rev.files.get_or_insert_with(Vec::new);
                if page == 1 {
                    files.clear();
                }
                let n = r.body.arr().len();
                files.extend(r.body.arr().iter().map(FileDiff::from_json));
                d.rev.ver += 1;
                if n >= 100 && page < 3 {
                    let req = Req::get(format!("/repos/{}/pulls/{}/files?per_page=100&page={}", d.item.repo, d.item.num, page + 1));
                    self.call(Tag::Files(g, page + 1), req);
                }
            }
            Tag::RComments(_) => {
                d.rev.comments = r.body.arr().iter().map(RComment::from_json).collect();
                d.rev.ver += 1;
            }
            Tag::Content(_, path) => {
                let lines = r.text.lines().map(String::from).collect();
                d.rev.contents.insert(path, Content::Text(lines));
                d.rev.ver += 1;
            }
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
        let Some(d) = &mut self.detail else { return };
        let (r, n) = (d.item.repo.clone(), d.item.num);
        let text = text.trim();
        if text.is_empty() && p != Purpose::Approve {
            return self.error("empty message");
        }
        let b = json::quote(text);
        let event = match &p {
            Purpose::Comment => return self.act("commented", Req::send("POST", format!("/repos/{}/issues/{}/comments", r, n), format!("{{\"body\":{}}}", b))),
            Purpose::Reply(id) => return self.act("replied", Req::send("POST", format!("/repos/{}/pulls/{}/comments/{}/replies", r, n, id), format!("{{\"body\":{}}}", b))),
            Purpose::Inline { path, line, start, right } => {
                let list = self.drafts.entry((r, n)).or_default();
                list.push(Pending { path: path.clone(), line: *line, start: *start, right: *right, body: text.to_string() });
                let k = list.len();
                d.rev.anchor = None;
                d.rev.ver += 1;
                return self.info(format!("comment queued ({} pending); S submits the review", k));
            }
            Purpose::Approve => ("APPROVE", "approved"),
            Purpose::Changes => ("REQUEST_CHANGES", "requested changes"),
            Purpose::Review => ("COMMENT", "reviewed"),
        };
        let sha = d.pull.path("head.sha").str().to_string();
        let mut comments = Vec::new();
        for c in self.drafts.get(&(r.clone(), n)).map(|v| v.as_slice()).unwrap_or(&[]) {
            let side = if c.right { "RIGHT" } else { "LEFT" };
            let start = c.start.map_or(String::new(), |s| format!(",\"start_line\":{},\"start_side\":\"{}\"", s, side));
            comments.push(format!("{{\"path\":{},\"line\":{},\"side\":\"{}\"{},\"body\":{}}}", json::quote(&c.path), c.line, side, start, json::quote(&c.body)));
        }
        let commit = if sha.is_empty() { String::new() } else { format!("\"commit_id\":{},", json::quote(&sha)) };
        let body = format!("{{{}\"event\":\"{}\",\"body\":{},\"comments\":[{}]}}", commit, event.0, b, comments.join(","));
        self.act(event.1, Req::send("POST", format!("/repos/{}/pulls/{}/reviews", r, n), body));
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
            Tag::RepoInfo => self.on_repo_info(res),
            Tag::Tree => self.on_tree(res),
            Tag::Code(path) => self.on_code(path, res),
            Tag::Blob(key) => {
                self.gallery.arrived(&key, res.map(|r| r.bytes), &self.screen);
                self.code.rev.ver += 1;
                if let Some(d) = &mut self.detail {
                    d.rev.ver += 1;
                }
            }
            Tag::Act(label) => match res {
                Ok(_) => {
                    if matches!(label, "approved" | "requested changes" | "reviewed") {
                        if let Some(d) = &self.detail {
                            self.drafts.remove(&(d.item.repo.clone(), d.item.num));
                        }
                    }
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
                Mode::Pick(_) | Mode::Files(_) => {}
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
            Mode::Files(mut p) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Files(p),
                Pick::Cancel => {}
                Pick::Accept => match (p.selected(), &mut self.detail) {
                    (Some(it), Some(d)) => d.rev.select_file(it.1),
                    (Some(it), None) => self.open_node(it.1, true),
                    _ => {}
                },
            },
            Mode::Help => {}
        }
    }

    fn normal_key(&mut self, k: Key) {
        let g = std::mem::take(&mut self.pending_g);
        if self.detail.is_some() {
            return self.detail_key(k, g);
        }
        if self.tab == CODE_TAB {
            return self.code_key(k, g);
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
            Key::Char(c @ '1'..='6') => self.set_tab(c as usize - '1' as usize),
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
        if self.detail.as_ref().is_some_and(|d| d.review) {
            return self.review_key(k, g);
        }
        let page = (self.screen.h.saturating_sub(2) / 2).max(1) as isize;
        let Some(d) = &mut self.detail else { return };
        let scroll = |d: &mut Detail, by: isize| d.scroll = (d.scroll as isize + by).max(0) as usize;
        match k {
            Key::Char('q') | Key::Esc | Key::Char('h') | Key::Left => self.close_detail(),
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
                d.review = true;
                if d.rev.files.is_none() {
                    self.fetch_review();
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

    fn mouse(&mut self, m: Mouse) {
        if self.detail.is_none() && self.tab == CODE_TAB {
            if m.y == 0 {
                if let MouseKind::Press(0) = m.kind {
                    if let Some(&(_, _, t)) = self.geo.tabs.iter().find(|&&(a, b, _)| (a..b).contains(&m.x)) {
                        self.set_tab(t);
                    }
                }
                return;
            }
            return self.code_mouse(m);
        }
        if self.detail.as_ref().is_some_and(|d| d.review) {
            return self.review_mouse(m);
        }
        if let Some(d) = &mut self.detail {
            match m.kind {
                MouseKind::WheelUp => d.scroll = d.scroll.saturating_sub(3),
                MouseKind::WheelDown => d.scroll += 3,
                _ => {}
            }
            return;
        }
        match m.kind {
            MouseKind::WheelUp => self.scroll_list(-3),
            MouseKind::WheelDown => self.scroll_list(3),
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
                Purpose::Review => "review› ",
                Purpose::Inline { .. } => "line› ",
                Purpose::Reply(_) => "reply› ",
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

        match &self.detail {
            Some(d) if d.review => self.render_review(bottom),
            Some(_) => self.render_detail(bottom),
            None => {
                if self.tab == CODE_TAB {
                    self.render_code(bottom);
                } else {
                    self.screen.set_images(Vec::new());
                    self.render_list(bottom);
                }
            }
        }

        // status line
        let (tag, tag_bg) = match &self.mode {
            Mode::Filter(_) => (" FILTER ", 108),
            Mode::Compose(..) => (" COMPOSE ", 108),
            Mode::Confirm(_) => (" CONFIRM ", 203),
            Mode::Pick(_) | Mode::Files(_) => (" PICK ", 180),
            _ => match &self.detail {
                Some(d) if d.review => (" REVIEW ", 73),
                Some(_) => (" ITEM ", 110),
                None if self.tab == CODE_TAB => (" CODE ", 108),
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
        let drafts = self.detail.as_ref().and_then(|d| self.drafts.get(&(d.item.repo.clone(), d.item.num))).map_or(0, |v| v.len());
        let right = format!("{}{}{}{}? help ", if self.pending > 0 { "… " } else { "" }, if drafts > 0 { format!("{} pending  ", drafts) } else { String::new() }, self.me, if self.me.is_empty() { "" } else { "  " });
        self.screen.puts(w.saturating_sub(str_width(&right)), y_status, &right, Style::new(FG_DIM, BG_BAR, 0), w);

        match &mut self.mode {
            Mode::Pick(p) => {
                let (x, y) = p.draw(&mut self.screen, y_status);
                cursor = Some((x, y, true));
            }
            Mode::Files(p) => {
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
        if matches!(self.mode, Mode::Pick(_) | Mode::Files(_) | Mode::Help) {
            self.screen.set_images(Vec::new());
        }
        self.screen.flush(cursor);
    }

    fn draw_tabs(&mut self) {
        let w = self.screen.w;
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        self.geo.tabs.clear();
        let mut x = 0;
        for (i, name) in TABS.iter().enumerate() {
            let name = match (i, &self.repo) {
                (REPO_TAB, Some(r)) => r.rsplit('/').next().unwrap_or(r),
                _ => name,
            };
            let n = self.lists[i].items.iter().filter(|it| i != INBOX_TAB || it.unread).count();
            let s = format!(" {} {}{} ", i + 1, name, if n > 0 { format!(" {}", n) } else { String::new() });
            let cur = i == self.tab;
            let x1 = self.screen.puts(x, 0, &s, if cur { Style::new(16, ACCENT, BOLD) } else { Style::new(250, BG_BAR, 0) }, w);
            self.geo.tabs.push((x, x1, i));
            x = x1 + 1;
        }
    }

    fn render_list(&mut self, bottom: usize) {
        let w = self.screen.w;
        self.draw_tabs();
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

    /// Draw markdown text in a scrollable pane (columns x0..x0+w, rows y0..y0+h).
    fn draw_markdown(&mut self, src: &str, hard_breaks: bool, (x0, w): (usize, usize), (y0, h): (usize, usize), scroll: &mut usize) {
        enum R {
            S(Vec<mtui::markdown::Span>),
            I { x: usize, url: String, alt: String, i: usize, of: usize, id: u32 },
        }
        let images = self.gallery.enabled;
        let cols = |x: usize| (x0 + w).saturating_sub(x + 2).min(60);
        let opts = mtui::markdown::Opts { width: w.saturating_sub(3), images, hard_breaks };
        let mut rows: Vec<R> = Vec::new();
        let mut id = 0;
        for ml in mtui::markdown::render(src, &opts) {
            match ml.image {
                Some((alt, url)) => {
                    let x = x0 + 1 + ml.indent;
                    let of = self.gallery.size(&url, cols(x), 14).map_or(1, |s| s.1);
                    id += 1;
                    rows.extend((0..of).map(|i| R::I { x, url: url.clone(), alt: alt.clone(), i, of, id }));
                }
                None => rows.push(R::S(ml.spans)),
            }
        }
        *scroll = (*scroll).min(rows.len().saturating_sub(h));
        let mut pics: HashMap<u32, (usize, usize, usize, usize, usize, String)> = HashMap::new();
        for (k, r) in rows.iter().enumerate().skip(*scroll).take(h) {
            let y = y0 + k - *scroll;
            match r {
                R::S(spans) => {
                    let mut x = x0 + 1;
                    for (t, st) in spans {
                        x = self.screen.puts(x, y, t, *st, x0 + w);
                    }
                }
                R::I { x, url, alt, i, of, id } => {
                    if self.gallery.size(url, cols(*x), 14).is_some() {
                        let e = pics.entry(*id).or_insert((y, *x, *i, *i, *of, url.clone()));
                        e.3 = i + 1;
                    } else {
                        if self.gallery.request(url) {
                            self.net.fetch_image(url, url);
                        }
                        let note = match self.gallery.slot(url) {
                            Some(mimg::Slot::Failed(e)) => format!(" ({})", e),
                            Some(mimg::Slot::Loading) => " …".to_string(),
                            _ => String::new(),
                        };
                        self.screen.puts(*x, y, &format!("🖼 {}{}", if alt.is_empty() { "image" } else { alt }, note), Style::new(C_LINK, 0, 0), x0 + w);
                    }
                }
            }
        }
        let mut ims = Vec::new();
        if !matches!(self.mode, Mode::Pick(_) | Mode::Files(_) | Mode::Help) {
            for (id, (y, x, i0, i1, of, url)) in pics {
                if let Some((c, _)) = self.gallery.size(&url, cols(x), 14) {
                    ims.extend(self.gallery.place(&url, id, (x, y), c, of, i0, i1));
                }
            }
        }
        self.screen.set_images(ims);
    }

    fn render_detail(&mut self, bottom: usize) {
        let w = self.screen.w;
        let images = self.gallery.enabled;
        let Some(d) = &mut self.detail else { return };
        let doc = d.doc();
        let view_h = bottom.saturating_sub(1);
        // text, markdown spans, or one line of an inline image
        enum R {
            T(usize, String, Style),
            S(usize, Vec<mtui::markdown::Span>),
            I { x: usize, url: String, alt: String, i: usize, of: usize, id: u32 },
        }
        let img_cols = |x: usize| w.saturating_sub(x + 2).min(60);
        let mut rows: Vec<R> = Vec::new();
        let mut next_id = 1;
        for l in &doc {
            if !l.md {
                for (a, b) in wrap(&l.text, w.saturating_sub(l.ind + 1).max(1)) {
                    rows.push(R::T(l.ind, l.text[a..b].to_string(), l.st));
                }
                continue;
            }
            let opts = mtui::markdown::Opts { width: w.saturating_sub(l.ind + 2), images, hard_breaks: true };
            for ml in mtui::markdown::render(&l.text, &opts) {
                match ml.image {
                    Some((alt, url)) => {
                        let x = 1 + l.ind + ml.indent;
                        let of = self.gallery.size(&url, img_cols(x), 14).map_or(1, |s| s.1);
                        next_id += 1;
                        rows.extend((0..of).map(|i| R::I { x, url: url.clone(), alt: alt.clone(), i, of, id: next_id }));
                    }
                    None => rows.push(R::S(l.ind, ml.spans)),
                }
            }
        }
        d.scroll = d.scroll.min(rows.len().saturating_sub(view_h));
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let title = format!(" {} #{} ", d.item.repo, d.item.num);
        self.screen.puts(0, 0, &title, Style::new(255, BG_BAR, BOLD), w);
        if rows.len() > view_h {
            let s = format!("{}% ", (d.scroll + view_h) * 100 / rows.len());
            self.screen.puts(w.saturating_sub(s.len()), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
        let mut pics: HashMap<u32, (usize, usize, usize, usize, usize, &str)> = HashMap::new();
        for (k, r) in rows.iter().enumerate().skip(d.scroll).take(view_h) {
            let y = 1 + k - d.scroll;
            match r {
                R::T(ind, t, st) => {
                    self.screen.puts(1 + ind, y, t, *st, w);
                }
                R::S(ind, spans) => {
                    let mut x = 1 + ind;
                    for (t, st) in spans {
                        x = self.screen.puts(x, y, t, *st, w);
                    }
                }
                R::I { x, url, alt, i, of, id } => {
                    if self.gallery.size(url, img_cols(*x), 14).is_some() {
                        let e = pics.entry(*id).or_insert((y, *x, *i, *i, *of, url));
                        e.3 = i + 1;
                    } else {
                        if self.gallery.request(url) {
                            self.net.fetch_image(url, url);
                        }
                        let note = match self.gallery.slot(url) {
                            Some(mimg::Slot::Failed(e)) => format!(" ({})", e),
                            Some(mimg::Slot::Loading) => " …".to_string(),
                            _ => String::new(),
                        };
                        let alt = if alt.is_empty() { "image" } else { alt };
                        self.screen.puts(*x, y, &format!("🖼 {}{}", alt, note), Style::new(C_LINK, 0, 0), w);
                    }
                }
            }
        }
        let mut ims = Vec::new();
        if !matches!(self.mode, Mode::Pick(_) | Mode::Files(_) | Mode::Help) {
            for (id, (y, x, i0, i1, of, url)) in pics {
                if let Some((cols, _)) = self.gallery.size(url, img_cols(x), 14) {
                    ims.extend(self.gallery.place(url, id, (x, y), cols, of, i0, i1));
                }
            }
        }
        self.screen.set_images(ims);
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
