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
        if let Key::Mouse(m) = k {
            match &self.mode {
                Mode::Normal => return self.mouse(m),
                Mode::Help | Mode::Image(_) if matches!(m.kind, MouseKind::Press(_)) => {
                    self.mode = Mode::Normal;
                    return;
                }
                Mode::Pick(..) | Mode::Links(_) | Mode::Atts(_) => {}
                _ => return,
            }
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.normal_key(k),
            Mode::Search(mut e) => match k {
                Key::Esc | Key::Ctrl('c') => {}
                Key::Enter => {
                    let q = e.take();
                    self.run_search(&q);
                }
                k => {
                    e.key(&k);
                    self.mode = Mode::Search(e);
                }
            },
            Mode::Confirm(rows) => {
                if matches!(k, Key::Char('y') | Key::Char('Y')) {
                    self.do_op(Op::Delete, rows);
                }
            }
            Mode::Pick(mut p, kind, rows) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Pick(p, kind, rows),
                Pick::Cancel => {}
                Pick::Accept => {
                    if let Some(id) = p.selected().map(|x| x.id.clone()) {
                        match kind {
                            PickKind::Jump => self.set_view(View::Box(id)),
                            PickKind::Move => self.move_to(rows, &id, false),
                            PickKind::Label => self.move_to(rows, &id, true),
                        }
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
            Mode::Atts(mut p) => match p.key(&k) {
                Pick::Continue => self.mode = Mode::Atts(p),
                Pick::Cancel => {}
                Pick::Accept => {
                    if let Some(a) = p.selected().map(|x| x.att.clone()) {
                        self.save_att(&a);
                    }
                }
            },
            Mode::Send => match k {
                Key::Char('y') => self.finish_compose(true),
                Key::Char('d') => self.finish_compose(false),
                Key::Char('e') | Key::Enter => self.edit_again(),
                Key::Char('q') | Key::Esc => {
                    self.compose = None;
                    self.info("discarded");
                }
                _ => self.mode = Mode::Send,
            },
            Mode::Help | Mode::Image(_) => {}
        }
    }

    fn normal_key(&mut self, k: Key) {
        let g = std::mem::take(&mut self.pending_g);
        match k {
            Key::Ctrl('k') => return self.pick(PickKind::Jump, Vec::new()),
            Key::Ctrl('r') => return self.sync(),
            Key::Char('?') => return self.mode = Mode::Help,
            _ => {}
        }
        let side = self.focus == Focus::Side && self.geo.side.w > 0;
        if self.thread.is_some() {
            return if side { self.thread_side_key(k) } else { self.thread_key(k, g) };
        }
        if side {
            return self.side_key(k);
        }
        self.list_key(k, g)
    }

    fn pick(&mut self, kind: PickKind, rows: Vec<Row>) {
        if kind != PickKind::Jump && rows.is_empty() {
            return;
        }
        let here = self.cur_box().map(String::from);
        let items: Vec<MbPick> = self.boxes.iter().filter(|b| kind == PickKind::Jump || here.as_deref() != Some(b.id.as_str())).map(|b| MbPick { id: b.id.clone(), label: self.box_path(&b.id) }).collect();
        let title = match kind {
            PickKind::Jump => "go to",
            PickKind::Move => "move to",
            PickKind::Label => "label",
        };
        self.mode = Mode::Pick(Picker::new(title, items), kind, rows);
    }

    pub(crate) fn move_sel(&mut self, d: isize) {
        let n = self.rows.len() as isize;
        self.sel = (self.sel as isize + d).clamp(0, (n - 1).max(0)) as usize;
        if self.sel + 15 >= self.rows.len() {
            self.load_more();
        }
    }

    /// Keys for acting on mail, shared by the list and a conversation.
    /// Returns true if the key was one of them.
    fn action_key(&mut self, k: &Key) -> bool {
        match k {
            Key::Char('e') => {
                let t = self.targets();
                self.do_op(Op::Archive, t)
            }
            Key::Char('#') | Key::Char('d') => {
                let t = self.targets();
                let in_trash = self.cur_box().is_some() && self.cur_box() == self.role("trash").map(|b| b.id.as_str());
                if in_trash {
                    if !t.is_empty() {
                        self.mode = Mode::Confirm(t);
                    }
                } else {
                    self.do_op(Op::Trash, t)
                }
            }
            Key::Char('!') => {
                let t = self.targets();
                self.do_op(Op::Spam, t)
            }
            Key::Char('u') => {
                let t = self.targets();
                self.do_op(Op::Read, t);
                if self.thread.as_ref().is_some_and(|t| t.loaded) && self.thread_row().is_some_and(|r| r.unread) {
                    self.close_thread();
                }
            }
            Key::Char('s') => {
                let t = self.targets();
                self.do_op(Op::Star, t)
            }
            Key::Char('m') => {
                let t = self.targets();
                self.pick(PickKind::Move, t)
            }
            Key::Char('M') => {
                let t = self.targets();
                self.pick(PickKind::Label, t)
            }
            Key::Char('c') => {
                let d = compose::new_mail(&self.ids, "");
                self.start_compose(d)
            }
            Key::Char('/') => self.mode = Mode::Search(LineEdit::new()),
            Key::Char('b') => self.show_side = !self.show_side,
            _ => return false,
        }
        true
    }

    fn list_key(&mut self, k: Key, g: bool) {
        if self.action_key(&k) {
            return;
        }
        let page = (self.geo.rows / 2).max(1) as isize;
        match k {
            Key::Char('q') => self.quit = true,
            Key::Char('j') | Key::Down | Key::Ctrl('n') => self.move_sel(1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => self.move_sel(-1),
            Key::Ctrl('d') | Key::PageDown => self.move_sel(page),
            Key::Ctrl('u') | Key::PageUp => self.move_sel(-page),
            Key::Char('G') | Key::End => {
                self.move_sel(isize::MAX / 2);
                self.load_more();
            }
            Key::Home => self.move_sel(isize::MIN / 2),
            Key::Char('g') if g => self.move_sel(isize::MIN / 2),
            Key::Char('g') => self.pending_g = true,
            Key::Char('J') | Key::Char('L') => self.step_folder(1),
            Key::Char('K') | Key::Char('H') => self.step_folder(-1),
            Key::Enter | Key::Char('l') | Key::Right => self.open_row(self.sel, None),
            Key::Char('r') => self.open_row(self.sel, Some(Then::Reply)),
            Key::Char('R') => self.open_row(self.sel, Some(Then::ReplyAll)),
            Key::Char('f') => self.open_row(self.sel, Some(Then::Forward)),
            Key::Char('x') | Key::Char(' ') => {
                if let Some(r) = self.rows.get(self.sel) {
                    let t = r.thread.clone();
                    if !self.marked.remove(&t) {
                        self.marked.insert(t);
                    }
                    self.move_sel(1);
                }
            }
            Key::Tab | Key::Char('h') | Key::Left => {
                self.show_side = true;
                self.focus = Focus::Side;
            }
            Key::Esc => {
                if !self.marked.is_empty() {
                    self.marked.clear();
                } else if matches!(self.view, Some(View::Search(_))) {
                    self.leave_search();
                }
            }
            _ => {}
        }
    }

    fn side_key(&mut self, k: Key) {
        if self.action_key(&k) {
            return;
        }
        let n = self.boxes.len() as isize;
        let mv = |s: &mut App, d: isize| s.side_sel = (s.side_sel as isize + d).clamp(0, (n - 1).max(0)) as usize;
        match k {
            Key::Char('q') => self.quit = true,
            Key::Char('J') | Key::Char('L') => self.step_folder(1),
            Key::Char('K') | Key::Char('H') => self.step_folder(-1),
            Key::Char('j') | Key::Down | Key::Ctrl('n') => mv(self, 1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => mv(self, -1),
            Key::Char('G') | Key::End => mv(self, isize::MAX / 2),
            Key::Home | Key::Char('g') => mv(self, isize::MIN / 2),
            Key::Enter | Key::Char('l') | Key::Right => {
                if let Some(b) = self.boxes.get(self.side_sel).map(|b| b.id.clone()) {
                    self.set_view(View::Box(b));
                }
                self.focus = Focus::List;
            }
            Key::Tab | Key::Esc => self.focus = Focus::List,
            _ => {}
        }
    }

    fn thread_key(&mut self, k: Key, g: bool) {
        if self.thread.as_ref().is_some_and(|t| !t.loaded) {
            if matches!(k, Key::Char('q') | Key::Esc | Key::Char('h') | Key::Left) {
                self.close_thread();
            }
            return;
        }
        if self.action_key(&k) {
            return;
        }
        let page = (self.screen.h.saturating_sub(3) / 2).max(1) as isize;
        let Some(t) = &mut self.thread else { return };
        let scroll = |t: &mut ThreadView, by: isize| t.scroll = (t.scroll as isize + by).max(0) as usize;
        let last = t.msgs.len().saturating_sub(1);
        match k {
            Key::Char('q') | Key::Esc | Key::Char('h') | Key::Left => self.close_thread(),
            Key::Char('j') | Key::Down | Key::Ctrl('n') => scroll(t, 1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => scroll(t, -1),
            Key::Ctrl('d') | Key::PageDown | Key::Char(' ') | Key::Ctrl('f') => scroll(t, page),
            Key::Ctrl('u') | Key::PageUp | Key::Ctrl('b') => scroll(t, -page),
            Key::Char('G') | Key::End => t.scroll = usize::MAX / 2,
            Key::Home => t.scroll = 0,
            Key::Char('g') if g => t.scroll = 0,
            Key::Char('g') => self.pending_g = true,
            Key::Char('n') => {
                t.cur = (t.cur + 1).min(last);
                t.follow = true;
            }
            Key::Char('p') => {
                t.cur = t.cur.saturating_sub(1);
                t.follow = true;
            }
            Key::Enter | Key::Char('l') | Key::Right => {
                if let Some(m) = t.msgs.get_mut(t.cur) {
                    m.expanded = !m.expanded;
                }
            }
            Key::Char('a') => {
                let open = t.msgs.iter().any(|m| !m.expanded);
                t.msgs.iter_mut().for_each(|m| m.expanded = open);
            }
            Key::Char('J') | Key::Char('L') => self.step_thread(1),
            Key::Char('K') | Key::Char('H') => self.step_thread(-1),
            Key::Tab if self.show_side => self.focus = Focus::Side,
            Key::Char('I') => match t.msgs.get(t.cur) {
                Some(m) if !m.remote.is_empty() => {
                    let id = m.mini.id.clone();
                    if !self.remote_ok.remove(&id) {
                        self.remote_ok.insert(id);
                    }
                }
                _ => self.info("no remote images in this message"),
            },
            Key::Char('r') | Key::Char('R') | Key::Char('f') | Key::Char('E') => {
                let Some(m) = t.msgs.get(t.cur) else { return };
                let d = match k {
                    Key::Char('r') => compose::reply(m, false, &self.ids),
                    Key::Char('R') => compose::reply(m, true, &self.ids),
                    Key::Char('f') => compose::forward(m, &self.ids),
                    _ if m.mini.has("$draft") => compose::from_draft(m),
                    _ => return self.info("not a draft"),
                };
                self.start_compose(d);
            }
            Key::Char('o') => {
                // this message's links, or the whole conversation's if it has none
                let mut links = t.msgs.get(t.cur).map(|m| m.links.clone()).unwrap_or_default();
                if links.is_empty() {
                    for l in t.msgs.iter().rev().flat_map(|m| m.links.iter()) {
                        if !links.contains(l) {
                            links.push(l.clone());
                        }
                    }
                }
                if links.is_empty() {
                    self.info("no links in this conversation");
                } else {
                    self.mode = Mode::Links(Picker::new("link", links));
                }
            }
            Key::Char('w') => match t.msgs.get(t.cur) {
                Some(m) if !m.atts.is_empty() => {
                    let items = m.atts.iter().map(|a| AttPick { att: a.clone(), label: format!("{}  ({})", a.name, model::human_size(a.size)) }).collect();
                    self.mode = Mode::Atts(Picker::new("save", items));
                }
                _ => self.info("no attachments in this message"),
            },
            _ => {}
        }
    }

    fn step_thread(&mut self, d: isize) {
        let i = self.sel as isize + d;
        if i < 0 || i >= self.rows.len() as isize {
            return;
        }
        let focus = self.focus;
        self.close_thread();
        self.open_row(i as usize, None);
        self.focus = focus;
        if self.sel + 15 >= self.rows.len() {
            self.load_more();
        }
    }

    /// The conversation list is the sidebar while one is open: moving in it
    /// switches conversation.
    fn thread_side_key(&mut self, k: Key) {
        match k {
            Key::Char('j') | Key::Down | Key::Ctrl('n') | Key::Char('J') | Key::Char('L') => self.step_thread(1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') | Key::Char('K') | Key::Char('H') => self.step_thread(-1),
            Key::Enter | Key::Char('l') | Key::Right | Key::Tab => self.focus = Focus::List,
            Key::Char('q') | Key::Esc | Key::Char('h') | Key::Left => self.close_thread(),
            k => self.thread_key(k, false),
        }
    }

    /// Switch to the next / previous folder.
    fn step_folder(&mut self, d: isize) {
        let at = self.cur_box().and_then(|b| self.boxes.iter().position(|m| m.id == b)).unwrap_or(self.side_sel);
        let to = (at as isize + d).clamp(0, self.boxes.len() as isize - 1);
        if to as usize != at || self.view.is_none() {
            if let Some(b) = self.boxes.get(to as usize).map(|b| b.id.clone()) {
                self.set_view(View::Box(b));
            }
        }
    }

    // ---- mouse ----

    fn mouse(&mut self, m: Mouse) {
        // the sidebar: folders, or the conversations while one is open
        if self.geo.side.contains(m.x) && m.y >= 1 {
            let thread = self.thread.is_some();
            match m.kind {
                MouseKind::WheelUp | MouseKind::WheelDown => {
                    let d = if m.kind == MouseKind::WheelUp { -3 } else { 3 };
                    if thread {
                        self.tside_st.scroll(d);
                    } else {
                        self.side_st.scroll(d);
                    }
                }
                MouseKind::Press(0) => {
                    let hit = self.geo.side.row_at(m.x, m.y).and_then(|r| self.geo.side_map.get(r).copied().flatten());
                    if let Some(i) = hit {
                        if thread {
                            self.focus = Focus::Side;
                            self.step_to(i);
                        } else if let Some(b) = self.boxes.get(i).map(|b| b.id.clone()) {
                            self.side_sel = i;
                            self.focus = Focus::Side;
                            self.set_view(View::Box(b));
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if self.thread.is_some() {
            return self.thread_mouse(m);
        }
        let x0 = self.geo.lx0;
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let d = if m.kind == MouseKind::WheelUp { -3isize } else { 3 };
                let n = self.rows.len();
                let rows = self.geo.rows.max(1);
                self.geo.top = (self.geo.top as isize + d).clamp(0, n.saturating_sub(rows) as isize) as usize;
                self.sel = self.sel.clamp(self.geo.top, (self.geo.top + rows).min(n).saturating_sub(1).max(self.geo.top));
                if self.geo.top + rows + 15 >= n {
                    self.load_more();
                }
            }
            MouseKind::Press(0) if m.y >= self.geo.y0 && m.y < self.geo.y0 + self.geo.rows => {
                let i = self.geo.top + m.y - self.geo.y0;
                let Some(r) = self.rows.get(i).cloned() else { return };
                self.sel = i;
                self.focus = Focus::List;
                // the left columns are buttons: mark, unread dot, star
                if m.x == x0 + 1 {
                    if !self.marked.remove(&r.thread) {
                        self.marked.insert(r.thread);
                    }
                    return;
                }
                if m.x == x0 + 2 {
                    return self.do_op(Op::Read, vec![r]);
                }
                if m.x == x0 + 3 {
                    return self.do_op(Op::Star, vec![r]);
                }
                let dbl = self.last_click.is_some_and(|(t, j)| j == i && t.elapsed() < Duration::from_millis(400));
                self.last_click = Some((Instant::now(), i));
                if dbl {
                    self.open_row(i, None);
                }
            }
            MouseKind::Press(2) if m.y >= self.geo.y0 && m.y < self.geo.y0 + self.geo.rows => {
                // right click: mark the row
                if let Some(r) = self.rows.get(self.geo.top + m.y - self.geo.y0) {
                    let t = r.thread.clone();
                    if !self.marked.remove(&t) {
                        self.marked.insert(t);
                    }
                }
            }
            _ => {}
        }
    }

    /// Open the list row `i` (from a sidebar click) keeping the sidebar focused.
    fn step_to(&mut self, i: usize) {
        if i >= self.rows.len() {
            return;
        }
        let focus = self.focus;
        self.close_thread();
        self.open_row(i, None);
        self.focus = focus;
    }

    fn thread_mouse(&mut self, m: Mouse) {
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                if let Some(t) = &mut self.thread {
                    t.scroll = if m.kind == MouseKind::WheelUp { t.scroll.saturating_sub(3) } else { t.scroll + 3 };
                }
            }
            MouseKind::Press(0) if m.y >= 1 => {
                self.focus = Focus::List;
                if let Some(k) = self.screen.image_at(m.x, m.y).and_then(|id| self.gallery.key_of(id)) {
                    self.mode = Mode::Image(k.to_string());
                    return;
                }
                let Some(&(i, hit)) = self.geo.trows.get(m.y - 1) else { return };
                let Some(t) = &mut self.thread else { return };
                if i >= t.msgs.len() {
                    return;
                }
                t.cur = i;
                match hit {
                    Hit::Head => t.msgs[i].expanded = !t.msgs[i].expanded,
                    Hit::Att(a) => {
                        if let Some(att) = t.msgs[i].atts.get(a).cloned() {
                            self.save_att(&att);
                        }
                    }
                    Hit::Links => {
                        let links = t.msgs[i].links.clone();
                        if !links.is_empty() {
                            self.mode = Mode::Links(Picker::new("link", links));
                        }
                    }
                    Hit::None => {}
                }
            }
            _ => {}
        }
    }
}
