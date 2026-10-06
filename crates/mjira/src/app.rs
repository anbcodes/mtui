// mjira state and calls. Lists are JQL searches; the server stays the source
// of truth: an action is sent, the screen is patched at once where it's
// obvious (a status, an assignee), and the next refresh replaces the guess
// with what Jira says. Jira has no push for clients, so lists refresh every
// minute and the open issue every half minute.

mod draw;
mod keys;

use crate::adf;
use crate::api::{self, Flavor, Net, Reply, Req, Tag, Why};
use crate::model::{self, Column, Issue};
use mtui::json::{quote, Value};
use mtui::lineedit::LineEdit;
use mtui::picker::{Label, Picker};
use mtui::screen::Screen;
use mtui::sidebar;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(60);
const DETAIL_REFRESH: Duration = Duration::from_secs(30);
const PAGE: usize = 50;
const BOARD_PAGE: usize = 100;

pub const TABS: [&str; 6] = ["mine", "watching", "recent", "project", "board", "search"];
pub const PROJECT_TAB: usize = 3;
pub const BOARD_TAB: usize = 4;
pub const SEARCH_TAB: usize = 5;

const DETAIL_FIELDS: &str = "summary,description,status,issuetype,priority,assignee,reporter,labels,components,fixVersions,created,updated,duedate,resolution,parent,subtasks,issuelinks,attachment,project,watches,timetracking,customfield_10016,customfield_10020";

pub const HELP: &str = "\
lists:
  j k ↑ ↓      select              gg G  first / last   C-d C-u  page
  Enter l      open                1-6 Tab H L  switch tab
  /            filter this list    s  search: JQL, words, or an issue key
  f            favourite filters   C-k  pick the project (project, board)
  t            move (transition)   a  assign   i  assign to me   p  priority
  n            new issue in the project
  o y          open in browser, copy URL (OSC 52)
  r            refresh             q  quit   C-z suspend
tabs: 1 mine, 2 watching, 3 recent, 4 project, 5 board, 6 search
board: h l ← → column  j k card   Enter open
  < >          move the card to the previous / next column
issue:
  j k C-d C-u  scroll              g G  top / bottom   Space  page
  J K (L H)    next / previous issue in the list; the list is the sidebar
               while you read (Tab moves there, j k switch)   b  hide it
  c            comment             e  edit the summary  #  labels
  t a i p      transition, assign, assign to me, priority
  w            watch / stop watching     W  log work (1h 30m, then a note)
  o y r        browser, copy URL, refresh        q Esc h  back
  Text you write: blank line = paragraph, - or 1. lists, ``` code, **bold**,
  `code`, [text](url) and bare links.
mouse: click a tab, card, row or sidebar issue (double-click opens), an image
       (fullscreen); wheel scrolls; shift+drag selects text (terminal)
compose: Enter sends   Alt-Enter newline   Esc cancels";

/// One choice in a picker.
#[derive(Clone)]
pub struct Opt {
    pub id: String,
    pub label: String,
    /// A status name and category for a transition's target, patched in at once.
    pub aux: (String, String),
}

impl Opt {
    fn new(id: impl Into<String>, label: impl Into<String>) -> Opt {
        Opt { id: id.into(), label: label.into(), aux: Default::default() }
    }
}

impl Label for Opt {
    fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, PartialEq)]
pub enum PickKind {
    Project,
    Filter,
    Transition(String),
    Assign(String),
    Priority(String),
    Type,
}

#[derive(Clone, PartialEq)]
pub enum Purpose {
    Comment,
    Summary,
    Labels,
    Worklog,
    /// A new issue of this type id.
    New(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Main,
    Side,
}

pub enum Mode {
    Normal,
    Filter(LineEdit),
    Jql,
    Compose(Purpose, LineEdit),
    Pick(Picker<Opt>, PickKind),
    Help,
    /// An image shown fullscreen, by gallery key.
    Image(String),
}

#[derive(Default)]
pub struct List {
    pub items: Vec<Issue>,
    pub loaded: bool,
    pub busy: bool,
    pub sel: usize,
    pub top: usize,
    /// Cursor for the next page, when there is one.
    pub next: Option<String>,
    /// Bumped when the query changes, so replies to the old one are dropped.
    pub seq: u64,
    pub err: Option<String>,
}

pub struct Detail {
    pub gen: u64,
    pub key: String,
    /// What the list knew, until the issue itself arrives.
    pub head: Issue,
    pub issue: Value,
    pub comments: Vec<Value>,
    pub scroll: usize,
    pub err: Option<String>,
    pub fetched: Instant,
}

#[derive(Default)]
pub struct Geo {
    pub tabs: Vec<(usize, usize, usize)>,
    pub y0: usize,
    pub rows: usize,
    pub top: usize,
}

/// Where the board's cards are on screen, for mouse hits.
#[derive(Default)]
pub struct BoardGeo {
    pub cw: usize,
    pub off: usize,
    pub y0: usize,
    pub card_h: usize,
    pub cols: Vec<Column>,
}

pub struct App {
    pub screen: Screen,
    net: Net,
    pub quit: bool,
    pub suspend: bool,
    gallery: mimg::Gallery,
    me: String,
    me_id: String,
    project: Option<String>,
    /// The project's statuses in workflow order, with their categories.
    statuses: Vec<(String, String)>,
    tab: usize,
    lists: [List; 6],
    filter: String,
    jql: String,
    jql_ed: LineEdit,
    detail: Option<Detail>,
    gen: u64,
    focus: Focus,
    side_on: bool,
    side_st: sidebar::State,
    side: sidebar::Geo,
    mode: Mode,
    msg: String,
    err: bool,
    pending: usize,
    last_refresh: Instant,
    geo: Geo,
    bgeo: BoardGeo,
    btops: Vec<usize>,
    /// The board selection the column scroll last followed.
    bseen: Option<usize>,
    last_click: Option<(Instant, usize)>,
    pending_g: bool,
}

impl App {
    pub fn new(w: usize, h: usize, net: Net, project: Option<String>, images: bool) -> App {
        let mut app = App {
            screen: Screen::new(w, h),
            net,
            quit: false,
            suspend: false,
            gallery: mimg::Gallery::new(images),
            me: String::new(),
            me_id: String::new(),
            project,
            statuses: Vec::new(),
            tab: 0,
            lists: Default::default(),
            filter: String::new(),
            jql: String::new(),
            jql_ed: LineEdit::new(),
            detail: None,
            gen: 0,
            focus: Focus::Main,
            side_on: true,
            side_st: Default::default(),
            side: Default::default(),
            mode: Mode::Normal,
            msg: "connecting…".into(),
            err: false,
            pending: 0,
            last_refresh: Instant::now(),
            geo: Geo::default(),
            bgeo: BoardGeo::default(),
            btops: Vec::new(),
            bseen: None,
            last_click: None,
            pending_g: false,
        };
        app.call(Tag::Me, Req::get("/myself"));
        if let Some(p) = app.project.clone() {
            app.call(Tag::Statuses(p.clone()), Req::get(format!("/project/{}/statuses", mhttp::urlencode(&p))));
        }
        app.refresh_all();
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

    fn flavor(&self) -> Flavor {
        self.net.flavor()
    }

    pub fn url_of(&self, key: &str) -> String {
        format!("{}/browse/{}", self.net.base(), key)
    }

    // ---- lists ----

    fn jql_for(&self, tab: usize) -> Option<String> {
        let proj = |p: &str| format!("project = {}", quote(p));
        Some(match tab {
            0 => "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC".into(),
            1 => "watcher = currentUser() AND statusCategory != Done ORDER BY updated DESC".into(),
            2 => "issuekey in issueHistory() ORDER BY lastViewed DESC".into(),
            PROJECT_TAB => format!("{} AND statusCategory != Done ORDER BY updated DESC", proj(self.project.as_ref()?)),
            BOARD_TAB => format!("{} AND (statusCategory != Done OR resolutiondate >= -14d) ORDER BY updated DESC", proj(self.project.as_ref()?)),
            _ if self.jql.is_empty() => return None,
            _ => self.jql.clone(),
        })
    }

    fn fetch(&mut self, i: usize, append: bool) {
        let l = &self.lists[i];
        if l.busy || (append && l.next.is_none()) {
            return;
        }
        let Some(jql) = self.jql_for(i) else { return };
        let page = if append { l.next.clone() } else { None };
        let seq = l.seq;
        self.lists[i].busy = true;
        let req = api::search(self.flavor(), &jql, page.as_deref(), if i == BOARD_TAB { BOARD_PAGE } else { PAGE });
        self.call(Tag::List(i, seq, append), req);
    }

    fn refresh_all(&mut self) {
        self.last_refresh = Instant::now();
        for i in 0..TABS.len() {
            self.fetch(i, false);
        }
    }

    fn reset_list(&mut self, i: usize) {
        let seq = self.lists[i].seq + 1;
        self.lists[i] = List { seq, ..List::default() };
    }

    fn on_list(&mut self, i: usize, seq: u64, append: bool, res: Result<api::Resp, String>) {
        if self.lists[i].seq != seq {
            return;
        }
        let l = &mut self.lists[i];
        l.busy = false;
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                l.loaded = true;
                l.err = Some(e.clone());
                return self.error(format!("{}: {}", TABS[i], e));
            }
        };
        l.err = None;
        l.loaded = true;
        l.next = api::next_page(self.net.flavor(), &r.body);
        let got = r.body.get("issues").arr().iter().map(Issue::from_json);
        if append {
            let seen: Vec<String> = l.items.iter().map(|x| x.key.clone()).collect();
            l.items.extend(got.filter(|x| !seen.contains(&x.key)));
        } else {
            let keep = self.cur_key_in(i);
            let l = &mut self.lists[i];
            l.items = got.collect();
            if let Some(k) = keep {
                self.select_key_in(i, &k);
            }
        }
        // With no project given, take the one most of my issues are in.
        if i == 0 && self.project.is_none() {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for it in &self.lists[0].items {
                match counts.iter_mut().find(|c| c.0 == it.project) {
                    Some(c) => c.1 += 1,
                    None => counts.push((it.project.clone(), 1)),
                }
            }
            if let Some((p, _)) = counts.into_iter().filter(|c| !c.0.is_empty()).max_by_key(|c| c.1) {
                self.set_project(&p, false);
            }
        }
        let n = self.nav_in(i).len();
        let l = &mut self.lists[i];
        l.sel = l.sel.min(n.saturating_sub(1));
    }

    /// Indices of the items the filter lets through.
    fn vis_in(&self, i: usize) -> Vec<usize> {
        let l = &self.lists[i];
        if self.filter.is_empty() || i != self.tab {
            return (0..l.items.len()).collect();
        }
        mtui::fuzzy::filter(&self.filter, l.items.iter().map(|it| it.label.as_str()))
    }

    pub fn columns(&self) -> Vec<Column> {
        model::columns(&self.statuses, &self.lists[BOARD_TAB].items, &self.vis_in(BOARD_TAB))
    }

    /// Items in the order j/k, J/K and the sidebar walk them: the list as
    /// shown, or the board column by column.
    fn nav_in(&self, i: usize) -> Vec<usize> {
        if i == BOARD_TAB {
            self.columns().into_iter().flat_map(|c| c.items).collect()
        } else {
            self.vis_in(i)
        }
    }

    fn nav(&self) -> Vec<usize> {
        self.nav_in(self.tab)
    }

    fn cur_key_in(&self, i: usize) -> Option<String> {
        let l = &self.lists[i];
        self.nav_in(i).get(l.sel).map(|&x| l.items[x].key.clone())
    }

    fn select_key_in(&mut self, i: usize, key: &str) {
        let pos = self.nav_in(i).iter().position(|&x| self.lists[i].items[x].key == key);
        if let Some(p) = pos {
            self.lists[i].sel = p;
        }
    }

    /// The issue under the cursor (the open one, in a detail view).
    fn cur_item(&self) -> Option<Issue> {
        if let Some(d) = &self.detail {
            return Some(d.head.clone());
        }
        let l = &self.lists[self.tab];
        self.nav().get(l.sel).map(|&i| l.items[i].clone())
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.nav().len() as isize;
        let tab = self.tab;
        let l = &mut self.lists[tab];
        l.sel = (l.sel as isize + d).clamp(0, (n - 1).max(0)) as usize;
        if tab != BOARD_TAB && l.sel + 10 >= l.items.len() {
            self.fetch(tab, true);
        }
    }

    /// Mouse wheel: scroll the view and pull the selection inside it, so the
    /// next draw doesn't scroll back to the selection.
    fn scroll_list(&mut self, d: isize) {
        let n = self.nav().len();
        let rows = self.geo.rows.max(1);
        let l = &mut self.lists[self.tab];
        l.top = (l.top as isize + d).clamp(0, n.saturating_sub(rows) as isize) as usize;
        l.sel = l.sel.clamp(l.top, (l.top + rows).min(n).saturating_sub(1).max(l.top));
        if self.tab != BOARD_TAB && l.top + rows + 10 >= l.items.len() {
            self.fetch(self.tab, true);
        }
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t % TABS.len();
        self.filter.clear();
        match self.tab {
            PROJECT_TAB | BOARD_TAB if self.project.is_none() => self.info("no project yet; pick one with C-k"),
            SEARCH_TAB if self.jql.is_empty() => self.info("press s to search with JQL, words or an issue key"),
            _ => {}
        }
    }

    fn set_project(&mut self, key: &str, show: bool) {
        self.project = Some(key.to_string());
        self.statuses.clear();
        self.reset_list(PROJECT_TAB);
        self.reset_list(BOARD_TAB);
        self.btops.clear();
        self.call(Tag::Statuses(key.into()), Req::get(format!("/project/{}/statuses", mhttp::urlencode(key))));
        self.fetch(PROJECT_TAB, false);
        self.fetch(BOARD_TAB, false);
        if show {
            self.detail = None;
            if self.tab != BOARD_TAB {
                self.set_tab(PROJECT_TAB);
            }
        }
    }

    fn run_search(&mut self, jql: String) {
        self.jql = jql;
        self.detail = None;
        self.reset_list(SEARCH_TAB);
        self.set_tab(SEARCH_TAB);
        self.fetch(SEARCH_TAB, false);
    }

    // ---- the open issue ----

    fn open_key(&mut self, key: &str, head: Option<Issue>) {
        self.gen += 1;
        let g = self.gen;
        self.call(Tag::Issue(g), Req::get(format!("/issue/{}?fields={}&expand=changelog", mhttp::urlencode(key), DETAIL_FIELDS)));
        self.call(Tag::Comments(g), Req::get(format!("/issue/{}/comment?orderBy=created&maxResults=100", mhttp::urlencode(key))));
        let head = head.unwrap_or_else(|| Issue { key: key.into(), ..Issue::default() });
        self.detail = Some(Detail { gen: g, key: key.into(), head, issue: Value::Null, comments: Vec::new(), scroll: 0, err: None, fetched: Instant::now() });
        self.info("");
    }

    fn open_item(&mut self, it: Issue) {
        let key = it.key.clone();
        self.open_key(&key, Some(it));
    }

    /// Fetch the open issue again, keeping the scroll position.
    fn reload_detail(&mut self) {
        let Some(d) = self.detail.take() else { return };
        self.open_key(&d.key, Some(d.head));
        if let Some(n) = &mut self.detail {
            n.scroll = d.scroll;
        }
    }

    /// Open the issue `d` places along the list.
    fn step_issue(&mut self, d: isize) {
        let Some(cur) = self.detail.as_ref().map(|d| d.key.clone()) else { return };
        let nav = self.nav();
        let items = &self.lists[self.tab].items;
        let Some(pos) = nav.iter().position(|&i| items[i].key == cur) else { return };
        let to = (pos as isize + d).clamp(0, nav.len() as isize - 1) as usize;
        if to == pos {
            return;
        }
        let it = items[nav[to]].clone();
        self.lists[self.tab].sel = to;
        self.open_item(it);
    }

    fn on_detail(&mut self, tag: Tag, res: Result<api::Resp, String>) {
        let (Tag::Issue(g) | Tag::Comments(g)) = tag else { return };
        let Some(d) = self.detail.as_mut().filter(|d| d.gen == g) else { return };
        let r = match res {
            Ok(r) => r,
            Err(e) => {
                d.err = Some(e);
                return;
            }
        };
        d.err = None;
        match tag {
            Tag::Issue(_) => {
                d.head = Issue::from_json(&r.body);
                d.issue = r.body;
                d.fetched = Instant::now();
                let h = d.head.clone();
                // keep the lists honest about what we just learned
                for l in self.lists.iter_mut() {
                    for x in l.items.iter_mut().filter(|x| x.key == h.key) {
                        *x = h.clone();
                    }
                }
            }
            _ => d.comments = r.body.get("comments").arr().to_vec(),
        }
    }

    pub fn close_detail(&mut self) {
        self.detail = None;
        self.focus = Focus::Main;
        self.info("");
        self.fetch(self.tab, false);
    }

    // ---- actions ----

    fn act(&mut self, label: &'static str, req: Req) {
        self.info(format!("{}…", label));
        self.call(Tag::Act(label), req);
    }

    fn set_fields(&mut self, label: &'static str, key: &str, fields: String) {
        self.act(label, Req::send("PUT", format!("/issue/{}", mhttp::urlencode(key)), format!("{{\"fields\":{{{}}}}}", fields)));
    }

    /// A text field's value as the API wants it: an ADF document on Cloud,
    /// the raw text on Server.
    fn text_json(&self, text: &str) -> String {
        match self.flavor() {
            Flavor::Cloud => adf::from_text(text),
            Flavor::Server => quote(text),
        }
    }

    /// The issue an action on a list applies to: the open one, else the selection.
    fn target(&self) -> Option<Issue> {
        self.cur_item().filter(|it| !it.key.is_empty())
    }

    fn want_transitions(&mut self, why: Why) {
        let key = match &why {
            Why::Pick(k) | Why::Move(k, _) => k.clone(),
        };
        self.info("loading transitions…");
        self.call(Tag::Transitions(why), Req::get(format!("/issue/{}/transitions", mhttp::urlencode(&key))));
    }

    fn transition(&mut self, key: &str, o: &Opt) {
        self.patch(key, |it| {
            if !o.aux.0.is_empty() {
                it.status = o.aux.0.clone();
                it.cat = o.aux.1.clone();
            }
        });
        self.act("moved", Req::send("POST", format!("/issue/{}/transitions", mhttp::urlencode(key)), format!("{{\"transition\":{{\"id\":{}}}}}", quote(&o.id))));
    }

    /// Apply a change to every copy of an issue we hold.
    fn patch(&mut self, key: &str, f: impl Fn(&mut Issue)) {
        for l in self.lists.iter_mut() {
            for x in l.items.iter_mut().filter(|x| x.key == key) {
                f(x);
            }
        }
        if let Some(d) = self.detail.as_mut().filter(|d| d.key == key) {
            f(&mut d.head);
        }
        // the card keeps the cursor wherever the patch moved it
        if self.tab == BOARD_TAB {
            self.select_key_in(BOARD_TAB, key);
        }
    }

    fn assign(&mut self, key: &str, id: &str, name: &str) {
        let field = if self.flavor() == Flavor::Cloud { "accountId" } else { "name" };
        let v = if id.is_empty() { "null".to_string() } else { quote(id) };
        let (id, name) = (id.to_string(), name.to_string());
        self.patch(key, |it| {
            it.assignee = name.clone();
            it.assignee_id = id.clone();
        });
        self.act("assigned", Req::send("PUT", format!("/issue/{}/assignee", mhttp::urlencode(key)), format!("{{\"{}\":{}}}", field, v)));
    }

    fn toggle_watch(&mut self) {
        let Some(d) = &self.detail else { return };
        let (key, on) = (d.key.clone(), d.issue.path("fields.watches.isWatching").bool());
        let who = if self.flavor() == Flavor::Cloud { "accountId" } else { "username" };
        let path = format!("/issue/{}/watchers", mhttp::urlencode(&key));
        if on {
            self.act("stopped watching", Req::send("DELETE", format!("{}?{}={}", path, who, mhttp::urlencode(&self.me_id)), String::new()));
        } else {
            self.act("watching", Req::send("POST", path, quote(&self.me_id)));
        }
    }

    fn compose(&mut self, p: Purpose) {
        let mut e = LineEdit::new();
        match (&p, &self.detail) {
            (Purpose::New(_), _) => {}
            (_, None) => return,
            (Purpose::Summary, Some(d)) => e.set(&d.head.summary),
            (Purpose::Labels, Some(d)) => e.set(&d.head.labels.join(" ")),
            _ => {}
        }
        self.mode = Mode::Compose(p, e);
    }

    fn new_issue(&mut self) {
        let Some(p) = self.project.clone() else { return self.error("no project; pick one with C-k") };
        self.info("loading issue types…");
        self.call(Tag::Types, Req::get(format!("/project/{}", mhttp::urlencode(&p))));
    }

    fn send_compose(&mut self, p: Purpose, text: &str) {
        let text = text.trim();
        let key = self.detail.as_ref().map(|d| d.key.clone()).unwrap_or_default();
        if text.is_empty() && p != Purpose::Labels {
            return self.error("nothing to send");
        }
        match p {
            Purpose::Comment => {
                let body = format!("{{\"body\":{}}}", self.text_json(text));
                self.act("commented", Req::send("POST", format!("/issue/{}/comment", mhttp::urlencode(&key)), body));
            }
            Purpose::Summary => {
                let s = text.to_string();
                self.patch(&key, |it| it.summary = s.clone());
                self.set_fields("summary changed", &key, format!("\"summary\":{}", quote(text)));
            }
            Purpose::Labels => {
                let labels: Vec<String> = text.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(quote).collect();
                self.set_fields("labels set", &key, format!("\"labels\":[{}]", labels.join(",")));
            }
            Purpose::Worklog => {
                let (spent, rest) = model::split_worklog(text);
                if spent.is_empty() {
                    return self.error("log work as a time like 1h 30m, then an optional note");
                }
                let note = if rest.is_empty() { String::new() } else { format!(",\"comment\":{}", self.text_json(rest)) };
                self.act("work logged", Req::send("POST", format!("/issue/{}/worklog", mhttp::urlencode(&key)), format!("{{\"timeSpent\":{}{}}}", quote(&spent.join(" ")), note)));
            }
            Purpose::New(ty) => {
                let Some(proj) = self.project.clone() else { return };
                let body = format!("{{\"fields\":{{\"project\":{{\"key\":{}}},\"issuetype\":{{\"id\":{}}},\"summary\":{}}}}}", quote(&proj), quote(&ty), quote(text));
                self.info("creating…");
                self.call(Tag::Created, Req::send("POST", "/issue", body));
            }
        }
    }

    pub fn open_url(&mut self, u: &str) {
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let r = std::process::Command::new(cmd).arg(u).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        match r {
            Ok(_) => self.info(format!("opened {}", u)),
            Err(e) => self.error(format!("{}: {}", cmd, e)),
        }
    }

    fn yank(&mut self) {
        if let Some(it) = self.target() {
            let u = self.url_of(&it.key);
            self.screen.osc52(&u);
            self.info(format!("copied {}", u));
        }
    }

    pub fn tick(&mut self) -> i32 {
        if !self.me.is_empty() && self.last_refresh.elapsed() >= REFRESH {
            self.refresh_all();
        }
        let idle = matches!(self.mode, Mode::Normal);
        if idle && self.pending == 0 && self.detail.as_ref().is_some_and(|d| d.fetched.elapsed() >= DETAIL_REFRESH) {
            self.reload_detail();
        }
        1000
    }

    fn pick(&mut self, title: &str, items: Vec<Opt>, kind: PickKind) {
        if matches!(self.mode, Mode::Normal) {
            self.info("");
            self.mode = Mode::Pick(Picker::new(title, items), kind);
        }
    }

    pub fn on_reply(&mut self, (tag, res): Reply) {
        self.pending = self.pending.saturating_sub(1);
        match tag {
            Tag::Me => match res {
                Ok(r) => {
                    self.me = r.body.get("displayName").opt_str().unwrap_or("me").to_string();
                    self.me_id = model::user_id(&r.body);
                    self.info(format!("signed in as {}", self.me));
                }
                Err(e) => self.error(format!("auth: {}", e)),
            },
            Tag::List(i, seq, append) => self.on_list(i, seq, append, res),
            Tag::Statuses(p) => {
                if let (Ok(r), true) = (res, self.project.as_deref() == Some(p.as_str())) {
                    let mut order: Vec<(String, String)> = Vec::new();
                    for ty in r.body.arr() {
                        for s in ty.get("statuses").arr() {
                            let n = s.get("name").str();
                            if !n.is_empty() && !order.iter().any(|o| o.0 == n) {
                                order.push((n.into(), s.path("statusCategory.key").str().into()));
                            }
                        }
                    }
                    self.statuses = order;
                }
            }
            Tag::Projects => match res {
                Ok(r) => {
                    let arr = if r.body.get("values").is_null() { r.body.arr() } else { r.body.get("values").arr() };
                    let items: Vec<Opt> = arr.iter().map(|p| Opt::new(p.get("key").str(), format!("{}  {}", p.get("key").str(), p.get("name").str()))).collect();
                    self.pick("project", items, PickKind::Project);
                }
                Err(e) => self.error(format!("projects: {}", e)),
            },
            Tag::Filters => match res {
                Ok(r) => {
                    let items: Vec<Opt> = r.body.arr().iter().filter(|f| !f.get("jql").str().is_empty()).map(|f| Opt::new(f.get("jql").str(), f.get("name").str())).collect();
                    if items.is_empty() {
                        self.info("no favourite filters (star some in Jira)");
                    } else {
                        self.pick("filter", items, PickKind::Filter);
                    }
                }
                Err(e) => self.error(format!("filters: {}", e)),
            },
            Tag::Transitions(why) => match res {
                Ok(r) => {
                    let opts: Vec<Opt> = r
                        .body
                        .get("transitions")
                        .arr()
                        .iter()
                        .map(|t| {
                            let (name, to) = (t.get("name").str(), t.path("to.name").str());
                            let label = if to.is_empty() || to == name { name.to_string() } else { format!("{}  → {}", name, to) };
                            Opt { id: t.get("id").str().into(), label, aux: (to.into(), t.path("to.statusCategory.key").str().into()) }
                        })
                        .collect();
                    match why {
                        Why::Pick(key) if opts.is_empty() => self.error(format!("{} has no transitions from here", key)),
                        Why::Pick(key) => self.pick("move", opts, PickKind::Transition(key)),
                        Why::Move(key, target) => match opts.iter().find(|o| o.aux.0.eq_ignore_ascii_case(&target) || o.label.eq_ignore_ascii_case(&target)) {
                            Some(o) => {
                                let o = o.clone();
                                self.transition(&key, &o);
                            }
                            None => self.error(format!("{} can't go to {} from its status", key, target)),
                        },
                    }
                }
                Err(e) => self.error(format!("transitions: {}", e)),
            },
            Tag::Users(key) => match res {
                Ok(r) => {
                    let mut opts = vec![Opt::new(self.me_id.clone(), format!("{} (me)", self.me)), Opt::new("", "Unassigned")];
                    let arr = r.body.arr().iter().filter(|u| model::user_id(u) != self.me_id && !matches!(u.get("active"), Value::Bool(false)));
                    opts.extend(arr.map(|u| Opt::new(model::user_id(u), u.get("displayName").str())));
                    self.pick("assign", opts, PickKind::Assign(key));
                }
                Err(e) => self.error(format!("users: {}", e)),
            },
            Tag::Priorities(key) => match res {
                Ok(r) => {
                    let items: Vec<Opt> = r.body.arr().iter().map(|p| Opt::new(p.get("id").str(), p.get("name").str())).collect();
                    self.pick("priority", items, PickKind::Priority(key));
                }
                Err(e) => self.error(format!("priorities: {}", e)),
            },
            Tag::Types => match res {
                Ok(r) => {
                    let items: Vec<Opt> = r.body.get("issueTypes").arr().iter().filter(|t| !t.get("subtask").bool()).map(|t| Opt::new(t.get("id").str(), t.get("name").str())).collect();
                    self.pick("type", items, PickKind::Type);
                }
                Err(e) => self.error(format!("issue types: {}", e)),
            },
            Tag::Created => match res {
                Ok(r) => {
                    let key = r.body.get("key").str().to_string();
                    self.info(format!("created {}", key));
                    self.open_key(&key, None);
                    self.refresh_all();
                }
                Err(e) => self.error(format!("create: {}", e)),
            },
            Tag::Image(key) => {
                self.gallery.arrived(&key, res.map(|r| r.bytes), &self.screen);
            }
            Tag::Act(label) => match res {
                Ok(_) => {
                    self.info(label);
                    if self.detail.is_some() {
                        self.reload_detail();
                    }
                    self.refresh_all();
                }
                Err(e) => {
                    // the optimistic patch was wrong: show what the server says
                    self.error(format!("{}: {}", label, e));
                    self.refresh_all();
                    if self.detail.is_some() {
                        self.reload_detail();
                    }
                }
            },
            t => self.on_detail(t, res),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_mentions_every_tab() {
        for t in TABS {
            assert!(HELP.contains(t), "{}", t);
        }
    }
}
