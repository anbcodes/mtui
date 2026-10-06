// Keys and mouse.

use super::*;
use mtui::picker::Pick;
use mtui::term::{Key, Mouse, MouseKind};

impl App {
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
            return self.pick_project();
        }
        if let Key::Mouse(m) = k {
            match &self.mode {
                Mode::Normal => return self.mouse(m),
                Mode::Help | Mode::Image(_) if matches!(m.kind, MouseKind::Press(_)) => {
                    self.mode = Mode::Normal;
                    return;
                }
                Mode::Pick(..) => {}
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
            Mode::Jql => match k {
                Key::Esc | Key::Ctrl('c') => {}
                Key::Enter => {
                    let q = self.jql_ed.take();
                    let q = q.trim();
                    if model::is_key(q) {
                        self.detail = None;
                        self.open_key(&q.to_uppercase(), None);
                    } else if !q.is_empty() {
                        self.run_search(model::to_jql(q));
                    }
                }
                k => {
                    self.jql_ed.key(&k);
                    self.mode = Mode::Jql;
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
            Mode::Pick(mut p, kind) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Pick(p, kind),
                Pick::Cancel => {}
                Pick::Accept => {
                    let typed = p.query.trim().to_uppercase();
                    match (p.selected().cloned(), kind) {
                        (Some(o), kind) => self.picked(kind, o),
                        (None, PickKind::Project) if !typed.is_empty() => self.set_project(&typed, true),
                        _ => {}
                    }
                }
            },
            Mode::Help | Mode::Image(_) => {}
        }
    }

    fn picked(&mut self, kind: PickKind, o: Opt) {
        match kind {
            PickKind::Project => self.set_project(&o.id, true),
            PickKind::Filter => self.run_search(o.id),
            PickKind::Transition(key) => self.transition(&key, &o),
            PickKind::Assign(key) => {
                let name = if o.id.is_empty() { String::new() } else if o.id == self.me_id { self.me.clone() } else { o.label.clone() };
                self.assign(&key, &o.id, &name)
            }
            PickKind::Priority(key) => {
                let label = o.label.clone();
                self.patch(&key, |it| it.priority = label.clone());
                self.set_fields("priority set", &key, format!("\"priority\":{{\"id\":{}}}", quote(&o.id)));
            }
            PickKind::Type => self.compose(Purpose::New(o.id)),
        }
    }

    fn pick_project(&mut self) {
        self.info("loading projects…");
        let path = if self.flavor() == Flavor::Cloud { "/project/search?maxResults=100&orderBy=key" } else { "/project" };
        self.call(Tag::Projects, Req::get(path));
    }

    fn normal_key(&mut self, k: Key) {
        let g = std::mem::take(&mut self.pending_g);
        if self.detail.is_some() {
            return self.detail_key(k, g);
        }
        if self.tab == BOARD_TAB && self.board_key(&k) {
            return;
        }
        self.list_key(k, g);
    }

    /// Keys that act on the issue under the cursor, in a list or an issue.
    fn issue_key(&mut self, k: &Key) -> bool {
        let Some(it) = self.target() else { return false };
        match k {
            Key::Char('t') => self.want_transitions(Why::Pick(it.key)),
            Key::Char('a') => {
                let path = format!("/user/assignable/search?issueKey={}&maxResults=100", mhttp::urlencode(&it.key));
                self.info("loading users…");
                self.call(Tag::Users(it.key), Req::get(path));
            }
            Key::Char('i') => {
                let (id, me) = (self.me_id.clone(), self.me.clone());
                self.assign(&it.key, &id, &me);
            }
            Key::Char('p') => {
                self.info("loading priorities…");
                self.call(Tag::Priorities(it.key), Req::get("/priority"));
            }
            Key::Char('o') => self.open_url(&self.url_of(&it.key)),
            Key::Char('y') => self.yank(),
            _ => return false,
        }
        true
    }

    fn list_key(&mut self, k: Key, g: bool) {
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
            Key::Char('s') => {
                self.jql_ed.clear();
                self.mode = Mode::Jql;
            }
            Key::Char('f') => {
                self.info("loading filters…");
                self.call(Tag::Filters, Req::get("/filter/favourite"));
            }
            Key::Char('n') => self.new_issue(),
            Key::Esc if !self.filter.is_empty() => self.filter.clear(),
            Key::Char('r') => self.refresh_all(),
            Key::Enter | Key::Char('l') | Key::Right => {
                if let Some(it) = self.cur_item() {
                    self.open_item(it);
                }
            }
            k => {
                self.issue_key(&k);
            }
        }
    }

    /// Board movement; false for keys the list handles.
    fn board_key(&mut self, k: &Key) -> bool {
        match k {
            Key::Char('h') | Key::Left => self.board_move(-1, 0),
            Key::Char('l') | Key::Right => self.board_move(1, 0),
            Key::Char('j') | Key::Down => self.board_move(0, 1),
            Key::Char('k') | Key::Up => self.board_move(0, -1),
            Key::Char('<') => self.move_card(-1),
            Key::Char('>') => self.move_card(1),
            _ => return false,
        }
        true
    }

    /// (column, row in it, flat index of the column's first card) of the selection.
    pub(super) fn board_pos(&self) -> Option<(usize, usize, Vec<usize>)> {
        let cols = self.columns();
        let sel = self.lists[BOARD_TAB].sel;
        let mut starts = Vec::new();
        let mut off = 0;
        let mut at = None;
        for (ci, c) in cols.iter().enumerate() {
            starts.push(off);
            if at.is_none() && sel < off + c.items.len() {
                at = Some((ci, sel - off));
            }
            off += c.items.len();
        }
        at.map(|(c, r)| (c, r, starts))
    }

    fn board_move(&mut self, dc: isize, dr: isize) {
        let cols = self.columns();
        let Some((ci, row, starts)) = self.board_pos() else { return };
        let to = if dr != 0 {
            starts[ci] + (row as isize + dr).clamp(0, cols[ci].items.len() as isize - 1) as usize
        } else {
            let mut c = ci as isize + dc;
            while c >= 0 && (c as usize) < cols.len() && cols[c as usize].items.is_empty() {
                c += dc;
            }
            if c < 0 || c as usize >= cols.len() {
                return;
            }
            starts[c as usize] + row.min(cols[c as usize].items.len() - 1)
        };
        self.lists[BOARD_TAB].sel = to;
    }

    fn move_card(&mut self, dc: isize) {
        let Some(it) = self.target() else { return };
        let cols = self.columns();
        let Some((ci, ..)) = self.board_pos() else { return };
        match cols.get((ci as isize + dc).max(0) as usize).filter(|_| ci as isize + dc >= 0) {
            Some(c) => self.want_transitions(Why::Move(it.key, c.name.clone())),
            None => self.info("no column that way"),
        }
    }

    fn detail_key(&mut self, k: Key, g: bool) {
        let page = (self.screen.h.saturating_sub(2) / 2).max(1) as isize;
        if self.focus == Focus::Side && self.side_on {
            match k {
                Key::Char('j') | Key::Down | Key::Ctrl('n') => return self.step_issue(1),
                Key::Char('k') | Key::Up | Key::Ctrl('p') => return self.step_issue(-1),
                Key::Tab | Key::Enter | Key::Char('l') | Key::Right | Key::Esc => return self.focus = Focus::Main,
                _ => {}
            }
        }
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
            Key::Char('J') | Key::Char('L') => self.step_issue(1),
            Key::Char('K') | Key::Char('H') => self.step_issue(-1),
            Key::Tab if self.side_on => self.focus = Focus::Side,
            Key::Char('b') => {
                self.side_on = !self.side_on;
                self.focus = Focus::Main;
            }
            Key::Char('c') => self.compose(Purpose::Comment),
            Key::Char('e') => self.compose(Purpose::Summary),
            Key::Char('#') => self.compose(Purpose::Labels),
            Key::Char('W') => self.compose(Purpose::Worklog),
            Key::Char('w') => self.toggle_watch(),
            Key::Char('r') => self.reload_detail(),
            k => {
                self.issue_key(&k);
            }
        }
    }

    // ---- mouse ----

    fn open_row(&mut self, i: usize) {
        let nav = self.nav();
        if let Some(&x) = nav.get(i) {
            let it = self.lists[self.tab].items[x].clone();
            self.lists[self.tab].sel = i;
            self.open_item(it);
        }
    }

    /// True when the click at flat index `i` follows a click on the same one.
    fn double(&mut self, i: usize) -> bool {
        let dbl = self.last_click.is_some_and(|(t, j)| j == i && t.elapsed() < Duration::from_millis(400));
        self.last_click = Some((Instant::now(), i));
        dbl
    }

    fn mouse(&mut self, m: Mouse) {
        if m.kind == MouseKind::Press(0) {
            if let Some(k) = self.screen.image_at(m.x, m.y).and_then(|id| self.gallery.key_of(id)) {
                self.mode = Mode::Image(k.to_string());
                return;
            }
        }
        if let Some(d) = &mut self.detail {
            if self.side.w > 0 && self.side.contains(m.x) {
                match m.kind {
                    MouseKind::WheelUp => self.side_st.scroll(-3),
                    MouseKind::WheelDown => self.side_st.scroll(3),
                    MouseKind::Press(0) => {
                        if let Some(r) = self.side.row_at(m.x, m.y) {
                            self.open_row(r);
                        }
                    }
                    _ => {}
                }
                return;
            }
            match m.kind {
                MouseKind::WheelUp => d.scroll = d.scroll.saturating_sub(3),
                MouseKind::WheelDown => d.scroll += 3,
                _ => {}
            }
            return;
        }
        if m.kind == MouseKind::Press(0) && m.y == 0 {
            if let Some(&(_, _, t)) = self.geo.tabs.iter().find(|&&(a, b, _)| (a..b).contains(&m.x)) {
                self.set_tab(t);
            }
            return;
        }
        if self.tab == BOARD_TAB {
            return self.board_mouse(m);
        }
        match m.kind {
            MouseKind::WheelUp => self.scroll_list(-3),
            MouseKind::WheelDown => self.scroll_list(3),
            MouseKind::Press(0) if m.y >= self.geo.y0 && m.y < self.geo.y0 + self.geo.rows => {
                let i = self.geo.top + m.y - self.geo.y0;
                if i < self.nav().len() {
                    self.lists[self.tab].sel = i;
                    if self.double(i) {
                        if let Some(it) = self.cur_item() {
                            self.open_item(it);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn board_mouse(&mut self, m: Mouse) {
        let g = &self.bgeo;
        if g.cw == 0 || m.y < g.y0 {
            return;
        }
        let col = g.off + m.x / (g.cw + 1);
        let Some(c) = g.cols.get(col) else { return };
        let top = self.btops.get(col).copied().unwrap_or(0);
        let idx = top + (m.y - g.y0) / g.card_h.max(1);
        let n = c.items.len();
        let start: usize = g.cols[..col].iter().map(|c| c.items.len()).sum();
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let by = if m.kind == MouseKind::WheelUp { -1 } else { 1 };
                if let Some(t) = self.btops.get_mut(col) {
                    *t = (*t as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
                }
            }
            MouseKind::Press(0) if idx < n => {
                let flat = start + idx;
                self.lists[BOARD_TAB].sel = flat;
                if self.double(flat) {
                    if let Some(it) = self.cur_item() {
                        self.open_item(it);
                    }
                }
            }
            _ => {}
        }
    }
}
