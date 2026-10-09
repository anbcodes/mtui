// Drawing.

use super::*;
use crate::timefmt;
use mtui::screen::{Style, BOLD, ITALIC};
use mtui::sidebar::Row;
use mtui::wrap::{str_width, wrap};
use std::collections::HashMap;

const FG_DIM: u8 = 242;
const BG_BAR: u8 = 236;
const BG_SEL: u8 = 237;
const ACCENT: u8 = 180;
const C_OK: u8 = 114;
const C_BAD: u8 = 203;
const C_WARN: u8 = 179;
const C_LINK: u8 = 75;
const NAME_COLORS: [u8; 12] = [167, 173, 179, 143, 107, 72, 74, 110, 104, 140, 175, 139];

fn name_color(s: &str) -> u8 {
    NAME_COLORS[s.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % NAME_COLORS.len()]
}

/// Colour of a status by Jira's category.
fn cat_color(cat: &str) -> u8 {
    match cat {
        "new" => 246,
        "done" => C_OK,
        _ => C_LINK,
    }
}

fn prio(p: &str) -> (&'static str, u8) {
    match p.to_lowercase().as_str() {
        "highest" | "critical" | "blocker" => ("▲▲", C_BAD),
        "high" | "major" => ("▲ ", 209),
        "medium" | "normal" => ("= ", C_WARN),
        "low" | "minor" => ("▼ ", 110),
        "lowest" | "trivial" => ("▼▼", 74),
        _ => ("  ", FG_DIM),
    }
}

fn kind_mark(kind: &str) -> (char, u8) {
    match kind.to_lowercase().as_str() {
        "bug" => ('B', C_BAD),
        "story" | "user story" => ('S', C_OK),
        "task" => ('T', C_LINK),
        "epic" => ('E', 140),
        "sub-task" | "subtask" => ('s', 110),
        k => (k.chars().next().map_or('·', |c| c.to_ascii_uppercase()), FG_DIM),
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}

/// A line of the open issue, before wrapping.
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

fn md(ind: usize, text: String) -> DL {
    DL { ind, text, st: Style::default(), md: true }
}

impl Detail {
    /// The attachments the text can refer to.
    fn media(&self) -> adf::Media {
        let items = self
            .issue
            .path("fields.attachment")
            .arr()
            .iter()
            .map(|a| adf::MediaItem { name: a.get("filename").str().into(), url: a.get("content").str().into(), image: matches!(a.get("mimeType").str(), "image/png" | "image/jpeg" | "image/jpg"), used: false })
            .collect();
        adf::Media { items }
    }

    fn doc(&self) -> Vec<DL> {
        let plain = Style::default();
        let dim = Style::new(FG_DIM, 0, 0);
        let head = Style::new(ACCENT, 0, BOLD);
        let f = self.issue.get("fields");
        let it = &self.head;
        let mut media = self.media();
        let mut d = Vec::new();
        d.push(dl(0, format!("{}  {}", it.key, it.summary), Style::new(255, 0, BOLD)));
        let mut line = vec![it.kind.clone(), it.priority.clone(), if it.assignee.is_empty() { "unassigned".into() } else { format!("→ {}", it.assignee) }];
        line.retain(|s| !s.is_empty());
        d.push(dl(0, format!("{}  {}", it.status, line.join(" · ")), Style::fg(cat_color(&it.cat))));
        let mut who = Vec::new();
        if !it.reporter.is_empty() {
            who.push(format!("reported by {}", it.reporter));
        }
        if !f.get("created").str().is_empty() {
            who.push(timefmt::ago(f.get("created").str()));
        }
        if !it.updated.is_empty() {
            who.push(format!("updated {}", timefmt::ago(&it.updated)));
        }
        if let Some(due) = f.get("duedate").opt_str() {
            who.push(format!("due {}", due));
        }
        if let Some(r) = f.path("resolution.name").opt_str() {
            who.push(format!("resolved: {}", r));
        }
        if !who.is_empty() {
            d.push(dl(0, who.join(" · "), dim));
        }
        let names = |v: &Value| v.arr().iter().map(|x| x.get("name").str().to_string()).collect::<Vec<_>>().join(", ");
        let mut meta: Vec<String> = Vec::new();
        if !it.labels.is_empty() {
            meta.push(format!("labels: {}", it.labels.join(", ")));
        }
        for (k, v) in [("components", f.get("components")), ("fix", f.get("fixVersions"))] {
            if !names(v).is_empty() {
                meta.push(format!("{}: {}", k, names(v)));
            }
        }
        let sprint = f.get("customfield_10020").arr().iter().filter(|s| s.get("state").str() != "closed").map(|s| s.get("name").str().to_string()).collect::<Vec<_>>().join(", ");
        if !sprint.is_empty() {
            meta.push(format!("sprint: {}", sprint));
        }
        if let Value::Num(p) = f.get("customfield_10016") {
            meta.push(format!("points: {}", p));
        }
        if let Some(t) = f.path("timetracking.timeSpent").opt_str() {
            meta.push(format!("logged: {}", t));
        }
        if f.path("watches.isWatching").bool() {
            meta.push("watching".into());
        }
        if !meta.is_empty() {
            d.push(dl(0, meta.join("   "), dim));
        }
        if let Some(p) = f.path("parent.key").opt_str() {
            d.push(dl(0, format!("in {}  {}", p, f.path("parent.fields.summary").str()), dim));
        }
        if let Some(e) = &self.err {
            d.push(dl(0, e.clone(), Style::fg(C_BAD)));
        }
        d.push(dl(0, "", plain));
        if self.issue.is_null() && self.err.is_none() {
            d.push(dl(0, "loading…", Style::new(FG_DIM, 0, ITALIC)));
            return d;
        }
        let body = adf::field_md(f.get("description"), &mut media);
        if body.trim().is_empty() {
            d.push(dl(0, "(no description)", Style::new(FG_DIM, 0, ITALIC)));
        } else {
            d.push(md(0, body));
        }

        // comments render before the attachment list so their inline media counts as shown
        let mut entries: Vec<(i64, String, String, String, Option<String>)> = Vec::new(); // when, who, iso, change, comment body
        for c in &self.comments {
            let body = adf::field_md(c.get("body"), &mut media);
            let iso = c.get("created").str();
            entries.push((timefmt::epoch(iso).unwrap_or(0), c.path("author.displayName").str().into(), iso.into(), String::new(), Some(body)));
        }
        for h in self.issue.path("changelog.histories").arr() {
            let iso = h.get("created").str();
            for i in h.get("items").arr() {
                let field = i.get("field").str();
                if !matches!(field, "status" | "assignee" | "resolution" | "priority" | "summary") {
                    continue;
                }
                let (a, b) = (i.get("fromString").str(), i.get("toString").str());
                let change = format!("{}: {} → {}", field, if a.is_empty() { "none" } else { a }, if b.is_empty() { "none" } else { b });
                entries.push((timefmt::epoch(iso).unwrap_or(0), h.path("author.displayName").str().into(), iso.into(), change, None));
            }
        }
        entries.sort_by_key(|e| e.0);

        let atts = self.issue.path("fields.attachment").arr();
        if !atts.is_empty() {
            d.push(dl(0, "", plain));
            d.push(dl(0, format!("attachments ({})", atts.len()), head));
            for a in atts {
                let by = a.path("author.displayName").str();
                d.push(dl(2, format!("📎 {}  {}  {}", a.get("filename").str(), model::human_size(a.get("size").num() as u64), by), dim));
            }
            let extra: String = media.items.iter().filter(|m| m.image && !m.used).map(|m| format!("![{}]({})\n\n", m.name, m.url)).collect();
            if !extra.is_empty() {
                d.push(md(2, extra));
            }
        }
        let subs = f.get("subtasks").arr();
        if !subs.is_empty() {
            d.push(dl(0, "", plain));
            d.push(dl(0, format!("subtasks ({})", subs.len()), head));
            for s in subs {
                let cat = s.path("fields.status.statusCategory.key").str();
                d.push(dl(2, format!("{} {}  {}  {}", if cat == "done" { "☑" } else { "☐" }, s.get("key").str(), s.path("fields.summary").str(), s.path("fields.status.name").str()), Style::fg(if cat == "done" { FG_DIM } else { 250 })));
            }
        }
        let links = f.get("issuelinks").arr();
        if !links.is_empty() {
            d.push(dl(0, "", plain));
            d.push(dl(0, "links", head));
            for l in links {
                let (other, text) = match (l.get("outwardIssue"), l.get("inwardIssue")) {
                    (o, _) if !o.is_null() => (o, l.path("type.outward").str()),
                    (_, i) => (i, l.path("type.inward").str()),
                };
                d.push(dl(2, format!("{} {}  {}  [{}]", text, other.get("key").str(), other.path("fields.summary").str(), other.path("fields.status.name").str()), Style::fg(250)));
            }
        }
        d.push(dl(0, "", plain));
        d.push(dl(0, if entries.is_empty() { "no activity yet".to_string() } else { "activity".to_string() }, head));
        for (_, who, iso, change, body) in entries {
            let age = timefmt::ago(&iso);
            match body {
                Some(b) => {
                    d.push(dl(0, "", plain));
                    d.push(dl(0, format!("{}  {}", who, age), Style::new(name_color(&who), 0, BOLD)));
                    if !b.trim().is_empty() {
                        d.push(md(2, b));
                    }
                }
                None => d.push(dl(2, format!("{} {}  {}", who, age, change), dim)),
            }
        }
        d
    }
}

impl App {
    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if w < 20 || h < 5 {
            self.screen.puts(0, 0, "too small", Style::default(), w);
            self.screen.flush(None);
            return;
        }
        let y_status = h - 1;
        if let Mode::Image(key) = &self.mode {
            self.screen.set_images(self.gallery.place_full(key, 1, (w, h - 1)).into_iter().collect());
            self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
            let x = self.screen.puts(0, y_status, " IMAGE ", Style::new(16, 110, BOLD), w) + 1;
            self.screen.puts(x, y_status, "any key or click to close", Style::new(250, BG_BAR, 0), w);
            self.screen.flush(None);
            return;
        }
        let mut cursor = None;
        let mut bottom = y_status; // first row not available to the body

        if let Mode::Compose(p, e) = &self.mode {
            let prompt = match p {
                Purpose::Comment => "comment› ",
                Purpose::Summary => "summary› ",
                Purpose::Labels => "labels› ",
                Purpose::Worklog => "log work› ",
                Purpose::Description => "description› ",
                Purpose::EditComment(_) => "edit comment› ",
                Purpose::New(_) => "new issue› ",
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

        self.side = Default::default();
        if self.detail.is_some() {
            self.render_detail(bottom);
        } else {
            self.screen.set_images(Vec::new());
            if self.tab == BOARD_TAB {
                self.render_board(bottom);
            } else {
                self.render_list(bottom);
            }
        }

        // status line
        let (tag, tag_bg) = match &self.mode {
            Mode::Filter(_) => (" FILTER ", 108),
            Mode::Jql => (" SEARCH ", 108),
            Mode::Compose(..) => (" COMPOSE ", 108),
            Mode::Pick(..) => (" PICK ", 180),
            _ if self.detail.is_some() => (" ISSUE ", 110),
            _ if self.tab == BOARD_TAB => (" BOARD ", 73),
            _ => (" LIST ", 110),
        };
        self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
        let x = self.screen.puts(0, y_status, tag, Style::new(16, tag_bg, BOLD), w) + 1;
        match &self.mode {
            Mode::Filter(e) => {
                let x2 = self.screen.puts(x, y_status, "/", Style::new(ACCENT, BG_BAR, 0), w);
                let (cx, cy) = e.draw(&mut self.screen, x2, y_status, w.saturating_sub(x2), 1, Style::new(255, BG_BAR, 0));
                cursor = Some((cx, cy, true));
            }
            Mode::Jql => {
                let x2 = self.screen.puts(x, y_status, "jql› ", Style::new(ACCENT, BG_BAR, 0), w);
                let (cx, cy) = self.jql_ed.draw(&mut self.screen, x2, y_status, w.saturating_sub(x2), 1, Style::new(255, BG_BAR, 0));
                cursor = Some((cx, cy, true));
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
            Mode::Pick(p, _) => {
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
                    let st = if l.ends_with(':') || l.starts_with("tabs:") || l.starts_with("board:") || l.starts_with("mouse:") || l.starts_with("compose:") { Style::new(ACCENT, BG_BAR, BOLD) } else { Style::new(252, BG_BAR, 0) };
                    self.screen.puts(bx + 2, by + i, l, st, bx + bw);
                }
                cursor = None;
            }
            _ => {}
        }
        if matches!(self.mode, Mode::Pick(..) | Mode::Help) {
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
            let name = match (i, &self.project) {
                (PROJECT_TAB, Some(p)) => p.as_str(),
                _ => name,
            };
            let n = if i <= 1 { self.lists[i].items.len() } else { 0 };
            let s = format!(" {} {}{} ", i + 1, name, if n > 0 { format!(" {}", n) } else { String::new() });
            let x1 = self.screen.puts(x, 0, &s, if i == self.tab { Style::new(16, ACCENT, BOLD) } else { Style::new(250, BG_BAR, 0) }, w);
            self.geo.tabs.push((x, x1, i));
            x = x1 + 1;
        }
        if let Some(p) = &self.project {
            let s = format!("{} ", p);
            if w > x + s.len() + 1 {
                self.screen.puts(w - s.len() - 1, 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
            }
        }
    }

    /// Message for a list with nothing to show.
    fn empty_text(&self) -> &'static str {
        let l = &self.lists[self.tab];
        if l.err.is_some() {
            "couldn't load (see the status line); r retries"
        } else if self.jql_for(self.tab).is_none() {
            if self.tab == SEARCH_TAB { "press s to search with JQL, words or an issue key" } else { "no project; press C-k to pick one" }
        } else if !l.loaded {
            "loading…"
        } else if !self.filter.is_empty() {
            "no matches"
        } else {
            "nothing here"
        }
    }

    fn render_list(&mut self, bottom: usize) {
        let w = self.screen.w;
        self.draw_tabs();
        let nav = self.nav();
        let (y0, rows) = (1, bottom.saturating_sub(1));
        let empty = self.empty_text();
        let l = &mut self.lists[self.tab];
        l.sel = l.sel.min(nav.len().saturating_sub(1));
        if l.sel < l.top {
            l.top = l.sel;
        }
        if l.sel >= l.top + rows {
            l.top = l.sel + 1 - rows;
        }
        l.top = l.top.min(nav.len().saturating_sub(rows));
        (self.geo.y0, self.geo.rows, self.geo.top) = (y0, rows, l.top);
        if nav.is_empty() {
            self.screen.puts(2, y0 + 1, empty, Style::new(FG_DIM, 0, ITALIC), w);
            return;
        }
        let items = &l.items;
        let key_w = nav.iter().map(|&i| items[i].key.len()).max().unwrap_or(4);
        let (show_who, show_prio) = (w >= 100, w >= 70);
        let right_w = 4 + 1 + 14 + if show_prio { 3 } else { 0 } + if show_who { 15 } else { 0 };
        let now = timefmt::now();
        for (k, &i) in nav.iter().enumerate().skip(l.top).take(rows) {
            let it = &items[i];
            let y = y0 + k - l.top;
            let sel = k == l.sel;
            let bg = if sel { BG_SEL } else { 0 };
            if sel {
                self.screen.fill(0, w, y, Style::new(0, bg, 0));
                self.screen.put(0, y, '▌', Style::new(ACCENT, bg, 0));
            }
            let (kc, kcol) = kind_mark(&it.kind);
            self.screen.put(2, y, kc, Style::new(kcol, bg, BOLD));
            self.screen.puts(4, y, &it.key, Style::new(FG_DIM, bg, 0), w);
            let x = 4 + key_w + 2;
            let rx = w.saturating_sub(right_w);
            let done = it.cat == "done";
            self.screen.puts(x, y, &it.summary, Style::new(if done { FG_DIM } else if sel { 255 } else { 252 }, bg, if sel { BOLD } else { 0 }), rx.saturating_sub(1).max(x));
            let mut cx = rx;
            if show_prio {
                let (g, c) = prio(&it.priority);
                self.screen.puts(cx, y, g, Style::new(c, bg, 0), w);
                cx += 3;
            }
            self.screen.puts(cx, y, &it.status, Style::new(cat_color(&it.cat), bg, 0), cx + 13);
            cx += 15;
            if show_who {
                self.screen.puts(cx, y, &it.assignee, Style::new(if it.assignee.is_empty() { FG_DIM } else { name_color(&it.assignee) }, bg, 0), cx + 14);
                cx += 15;
            }
            let age = timefmt::age_at(&it.updated, now);
            self.screen.puts(w.saturating_sub(age.len() + 1).max(cx), y, &age, Style::new(FG_DIM, bg, 0), w);
        }
    }

    fn render_board(&mut self, bottom: usize) {
        let w = self.screen.w;
        self.draw_tabs();
        let cols = self.columns();
        let y0 = 3;
        if cols.is_empty() {
            let empty = self.empty_text();
            self.screen.puts(2, 2, empty, Style::new(FG_DIM, 0, ITALIC), w);
            self.bgeo = BoardGeo::default();
            return;
        }
        let card_h = 4;
        let rows = bottom.saturating_sub(y0) / card_h;
        let shown = (w / 26).clamp(1, cols.len());
        let cw = (w + 1) / shown - 1;
        let Some((cur_col, cur_row, _)) = self.board_pos() else {
            self.bgeo = BoardGeo { cw, y0, card_h, cols, ..BoardGeo::default() };
            return;
        };
        // keep the selected column on screen
        let mut off = self.bgeo.off.min(cols.len() - shown);
        if cur_col < off {
            off = cur_col;
        }
        if cur_col >= off + shown {
            off = cur_col + 1 - shown;
        }
        self.btops.resize(cols.len(), 0);
        if self.bseen != Some(self.lists[BOARD_TAB].sel) {
            self.bseen = Some(self.lists[BOARD_TAB].sel);
            let t = &mut self.btops[cur_col];
            *t = (*t).min(cur_row);
            if cur_row >= *t + rows.max(1) {
                *t = cur_row + 1 - rows.max(1);
            }
        }
        let items = &self.lists[BOARD_TAB].items;
        for (ci, c) in cols.iter().enumerate().skip(off).take(shown) {
            let x = (ci - off) * (cw + 1);
            let selcol = ci == cur_col;
            let cc = cat_color(&c.cat);
            self.screen.fill(x, x + cw, 1, Style::new(16, cc, BOLD));
            self.screen.puts(x + 1, 1, &c.name.to_uppercase(), Style::new(16, cc, BOLD), x + cw);
            let n = c.items.len().to_string();
            self.screen.puts((x + cw).saturating_sub(n.len() + 1), 1, &n, Style::new(16, cc, BOLD), x + cw);
            if x + cw < w {
                for y in 1..bottom {
                    self.screen.put(x + cw, y, '│', Style::fg(237));
                }
            }
            let top = self.btops[ci].min(c.items.len().saturating_sub(1));
            self.btops[ci] = top;
            if c.items.len() > top + rows {
                self.screen.puts((x + cw).saturating_sub(3), bottom.saturating_sub(1), " ▾ ", Style::fg(FG_DIM), x + cw);
            }
            for (k, &i) in c.items.iter().enumerate().skip(top).take(rows) {
                let it = &items[i];
                let y = y0 + (k - top) * card_h;
                let sel = selcol && k == cur_row;
                let bg = if sel { BG_SEL } else { 0 };
                for r in 0..card_h - 1 {
                    self.screen.fill(x, x + cw, y + r, Style::new(0, bg, 0));
                }
                if sel {
                    for r in 0..card_h - 1 {
                        self.screen.put(x, y + r, '▌', Style::new(ACCENT, bg, 0));
                    }
                }
                let (kc, kcol) = kind_mark(&it.kind);
                self.screen.put(x + 1, y, kc, Style::new(kcol, bg, BOLD));
                self.screen.puts(x + 3, y, &it.key, Style::new(FG_DIM, bg, 0), x + cw);
                let who = initials(&it.assignee);
                let (g, pc) = prio(&it.priority);
                let right = format!("{} {}", g.trim_end(), who);
                self.screen.puts((x + cw).saturating_sub(str_width(&right) + 1), y, g.trim_end(), Style::new(pc, bg, 0), x + cw);
                let wx = (x + cw).saturating_sub(who.len() + 1);
                self.screen.puts(wx, y, &who, Style::new(if who.is_empty() { FG_DIM } else { name_color(&it.assignee) }, bg, BOLD), x + cw);
                let lines = wrap(&it.summary, cw.saturating_sub(3).max(1));
                for (r, &(a, b)) in lines.iter().take(2).enumerate() {
                    let mut t = it.summary[a..b].trim_end().to_string();
                    if r == 1 && lines.len() > 2 {
                        t.push('…');
                    }
                    self.screen.puts(x + 2, y + 1 + r, &t, Style::new(if sel { 255 } else { 250 }, bg, if sel { BOLD } else { 0 }), x + cw);
                }
            }
        }
        self.bgeo = BoardGeo { cw, off, y0, card_h, cols };
    }

    fn render_detail(&mut self, bottom: usize) {
        let w = self.screen.w;
        let images = self.gallery.enabled;
        let Some(d) = &self.detail else { return };
        let doc = d.doc();
        let (key, head, mut scroll) = (d.key.clone(), d.head.clone(), d.scroll);
        let view_h = bottom.saturating_sub(1);

        let side_w = if self.side_on && w >= 80 { (w / 4).clamp(24, 36) } else { 0 };
        if side_w > 0 {
            let nav = self.nav();
            let items = &self.lists[self.tab].items;
            let rows: Vec<Row> = nav
                .iter()
                .enumerate()
                .map(|(k, &i)| {
                    let it = &items[i];
                    Row { text: format!("{} {}", it.key, it.summary), mark: Some(('●', cat_color(&it.cat))), dim: it.cat == "done", current: it.key == key, target: Some(k), ..Row::default() }
                })
                .collect();
            let sel = nav.iter().position(|&i| items[i].key == key);
            self.side = sidebar::draw(&mut self.screen, &rows, sel, &mut self.side_st, (0, side_w), (1, view_h), self.focus == Focus::Side);
        }
        let x0 = if side_w > 0 { side_w + 1 } else { 0 };
        let mw = w - x0;

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
                for (a, b) in wrap(&l.text, mw.saturating_sub(l.ind + 2).max(1)) {
                    rows.push(R::T(l.ind, l.text[a..b].to_string(), l.st));
                }
                continue;
            }
            let opts = mtui::markdown::Opts { width: mw.saturating_sub(l.ind + 3), images, hard_breaks: true };
            for ml in mtui::markdown::render(&l.text, &opts) {
                match ml.image {
                    Some((alt, url)) => {
                        let x = x0 + 1 + l.ind + ml.indent;
                        let of = self.gallery.size(&url, img_cols(x), 14).map_or(1, |s| s.1);
                        next_id += 1;
                        rows.extend((0..of).map(|i| R::I { x, url: url.clone(), alt: alt.clone(), i, of, id: next_id }));
                    }
                    None => rows.push(R::S(l.ind, ml.spans)),
                }
            }
        }
        scroll = scroll.min(rows.len().saturating_sub(view_h));
        if let Some(d) = &mut self.detail {
            d.scroll = scroll;
        }
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let title = format!(" {} ", key);
        let tx = self.screen.puts(0, 0, &title, Style::new(255, BG_BAR, BOLD), w);
        self.screen.puts(tx, 0, &head.status, Style::new(cat_color(&head.cat), BG_BAR, 0), w);
        if rows.len() > view_h {
            let s = format!("{}% ", (scroll + view_h) * 100 / rows.len());
            self.screen.puts(w.saturating_sub(s.len()), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
        let mut pics: HashMap<u32, (usize, usize, usize, usize, usize, &str)> = HashMap::new();
        for (k, r) in rows.iter().enumerate().skip(scroll).take(view_h) {
            let y = 1 + k - scroll;
            match r {
                R::T(ind, t, st) => {
                    self.screen.puts(x0 + 1 + ind, y, t, *st, w);
                }
                R::S(ind, spans) => {
                    let mut x = x0 + 1 + ind;
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
        if !matches!(self.mode, Mode::Pick(..) | Mode::Help) {
            for (id, (y, x, i0, i1, of, url)) in pics {
                if let Some((cols, _)) = self.gallery.size(url, img_cols(x), 14) {
                    ims.extend(self.gallery.place(url, id, (x, y), cols, of, i0, i1));
                }
            }
        }
        self.screen.set_images(ims);
    }
}
