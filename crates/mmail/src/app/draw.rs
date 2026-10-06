// Drawing.

use super::*;
use crate::timefmt;
use mtui::screen::{Style, BOLD, ITALIC};
use mtui::sidebar;
use mtui::wrap::{str_width, wrap};

const FG_DIM: u8 = 242;
const BG_BAR: u8 = 236;
const BG_SEL: u8 = 237;
const ACCENT: u8 = 180;
const C_OK: u8 = 114;
const C_BAD: u8 = 203;
const C_WARN: u8 = 179;
const C_LINK: u8 = 75;
const C_STAR: u8 = 220;
const NAME_COLORS: [u8; 12] = [167, 173, 179, 143, 107, 72, 74, 110, 104, 140, 175, 139];

fn name_color(s: &str) -> u8 {
    NAME_COLORS[s.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % NAME_COLORS.len()]
}

/// One drawn line of a conversation.
struct DL {
    spans: Vec<(String, Style)>,
    right: String,
    msg: usize,
    /// Highlight as the message marker.
    head: bool,
    hit: Hit,
    /// One row of an inline image: its key, which row of how many, columns, and a per-frame id.
    img: Option<(String, usize, usize, usize, u32)>,
}

impl DL {
    fn new(spans: Vec<(String, Style)>, msg: usize) -> DL {
        DL { spans, right: String::new(), msg, head: false, hit: Hit::None, img: None }
    }
}

fn body_lines(text: &str, width: usize, msg: usize, out: &mut Vec<DL>) {
    let plain = Style::fg(252);
    let quote = Style::fg(109);
    let dim = Style::fg(FG_DIM);
    let mut sig = false;
    for l in text.trim_matches('\n').lines() {
        let l = l.trim_end();
        if l == "-- " || l == "--" {
            sig = true;
        }
        let (prefix, rest) = if l.starts_with('>') {
            let n = l.len() - l.trim_start_matches(['>', ' ']).len();
            (&l[..n], &l[n..])
        } else {
            ("", l)
        };
        let st = if sig { dim } else if prefix.is_empty() { plain } else { quote };
        let pw = str_width(prefix);
        if rest.is_empty() {
            out.push(DL::new(vec![(prefix.trim_end().to_string(), st)], msg));
            continue;
        }
        for (a, b) in wrap(rest, width.saturating_sub(pw).max(8)) {
            out.push(DL::new(vec![(format!("{}{}", prefix, &rest[a..b]), st)], msg));
        }
    }
}

impl App {
    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if w < 30 || h < 6 {
            self.screen.puts(0, 0, "too small", Style::default(), w);
            self.screen.flush(None);
            return;
        }
        let y_status = h - 1;
        let mut cursor = None;

        if let Mode::Image(key) = &self.mode {
            self.screen.set_images(self.gallery.place_full(key, 1, (w, h - 1)).into_iter().collect());
            self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
            let x = self.screen.puts(0, y_status, " IMAGE ", Style::new(16, 110, BOLD), w) + 1;
            self.screen.puts(x, y_status, "any key or click to close", Style::new(250, BG_BAR, 0), w);
            self.screen.flush(None);
            return;
        }
        self.draw_bar();
        self.screen.set_images(Vec::new());
        let thread = self.thread.is_some();
        let mut side_w = if !self.show_side { 0 } else if thread { (w / 3).clamp(28, 42) } else { (w / 4).clamp(16, 30) };
        if w < side_w + 40 {
            side_w = 0;
        }
        let x0 = if side_w > 0 { side_w + 1 } else { 0 };
        if side_w > 0 {
            self.render_side(side_w, y_status);
        } else {
            self.geo.side = Default::default();
            self.geo.side_map.clear();
            if !thread && self.focus == Focus::Side {
                self.focus = Focus::List;
            }
        }
        if thread {
            self.render_thread(x0, y_status);
        } else {
            self.render_list(x0, y_status);
        }

        // status line
        let (tag, tag_bg) = match &self.mode {
            Mode::Search(_) => (" SEARCH ", 108),
            Mode::Confirm(_) => (" CONFIRM ", 203),
            Mode::Pick(..) | Mode::Links(_) | Mode::Atts(_) => (" PICK ", 180),
            Mode::Send => (" SEND? ", 203),
            _ if self.thread.is_some() => (" MAIL ", 110),
            _ if self.focus == Focus::Side && self.geo.side.w > 0 => (if self.thread.is_some() { " LIST " } else { " FOLDERS " }, 108),
            _ => (" LIST ", 110),
        };
        self.screen.fill(0, w, y_status, Style::new(250, BG_BAR, 0));
        let live = if self.live { ("● live", C_OK) } else { ("○ polling", C_WARN) };
        let right = format!("{}{}  {}  ? help ", if self.pending > 0 { "… " } else { "" }, live.0, self.me);
        let rx = w.saturating_sub(str_width(&right));
        let x = self.screen.puts(0, y_status, tag, Style::new(16, tag_bg, BOLD), w) + 1;
        let w_left = rx.saturating_sub(1).max(x);
        match &self.mode {
            Mode::Search(e) => {
                let x2 = self.screen.puts(x, y_status, "/", Style::new(ACCENT, BG_BAR, 0), w);
                let (cx, cy) = e.draw(&mut self.screen, x2, y_status, w_left.saturating_sub(x2), 1, Style::new(255, BG_BAR, 0));
                cursor = Some((cx, cy, true));
            }
            Mode::Confirm(rows) => {
                let n = rows.len();
                self.screen.puts(x, y_status, &format!("delete {} conversation{} for good? (y/n)", n, if n == 1 { "" } else { "s" }), Style::new(C_BAD, BG_BAR, BOLD), w_left);
            }
            Mode::Send => {
                let s = match &self.compose {
                    Some(c) => format!("y send  d draft  e edit  q discard — to {}: {}", if c.draft.to.is_empty() { &c.draft.cc } else { &c.draft.to }, c.draft.subject),
                    None => String::new(),
                };
                let st = if self.err { Style::new(C_BAD, BG_BAR, BOLD) } else { Style::new(255, BG_BAR, 0) };
                let s = if self.err { format!("{}  —  {}", self.msg, "y retry, e edit, q discard") } else { s };
                self.screen.puts(x, y_status, &s, st, w_left);
            }
            _ => {
                let st = Style::new(if self.err { C_BAD } else { 250 }, BG_BAR, 0);
                let marked = if self.marked.is_empty() { String::new() } else { format!("{} marked  ", self.marked.len()) };
                self.screen.puts(x, y_status, &format!("{}{}", marked, self.msg.lines().next().unwrap_or("")), st, w_left);
            }
        }
        self.screen.puts(rx, y_status, &right, Style::new(FG_DIM, BG_BAR, 0), w);
        self.screen.puts(rx + if self.pending > 0 { 2 } else { 0 }, y_status, live.0, Style::new(live.1, BG_BAR, 0), w);

        match &mut self.mode {
            Mode::Pick(p, ..) => cursor = Some({
                let (x, y) = p.draw(&mut self.screen, y_status);
                (x, y, true)
            }),
            Mode::Links(p) => cursor = Some({
                let (x, y) = p.draw(&mut self.screen, y_status);
                (x, y, true)
            }),
            Mode::Atts(p) => cursor = Some({
                let (x, y) = p.draw(&mut self.screen, y_status);
                (x, y, true)
            }),
            Mode::Help => {
                let lines: Vec<&str> = HELP.lines().collect();
                let bw = lines.iter().map(|l| str_width(l)).max().unwrap_or(0) + 4;
                let bx = w.saturating_sub(bw) / 2;
                let by = h.saturating_sub(lines.len() + 2) / 2;
                for (i, l) in std::iter::once("").chain(lines.iter().copied()).chain(std::iter::once("")).enumerate() {
                    self.screen.fill(bx, bx + bw, by + i, Style::new(252, BG_BAR, 0));
                    let head = l.ends_with(':') || l.starts_with("search:") || l.starts_with("writing:") || l.starts_with("mouse:");
                    let st = if head { Style::new(ACCENT, BG_BAR, BOLD) } else { Style::new(252, BG_BAR, 0) };
                    self.screen.puts(bx + 2, by + i, l, st, bx + bw);
                }
                cursor = None;
            }
            _ => {}
        }
        self.screen.flush(cursor);
    }

    fn draw_bar(&mut self) {
        let w = self.screen.w;
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let x = self.screen.puts(0, 0, " mmail ", Style::new(16, ACCENT, BOLD), w) + 1;
        let title = if let Some(t) = &self.thread {
            t.subject.clone()
        } else {
            match &self.view {
                Some(View::Box(b)) => self.box_path(b),
                Some(View::Search(q)) => format!("search: {}", q),
                None => "…".into(),
            }
        };
        let x = self.screen.puts(x, 0, &title, Style::new(255, BG_BAR, BOLD), w);
        if self.thread.is_none() && self.view.is_some() {
            let unread = self.rows.iter().filter(|r| r.unread).count();
            let s = if self.total > self.rows.len() { format!("  {}/{}", self.rows.len(), self.total) } else { format!("  {}", self.rows.len()) };
            let s = if unread > 0 { format!("{}  {} unread", s, unread) } else { s };
            self.screen.puts(x, 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
    }

    /// The sidebar: the folders, or, while a conversation is open, the
    /// conversations of the folder (so you can move through them).
    fn render_side(&mut self, sw: usize, bottom: usize) {
        let h = bottom.saturating_sub(1);
        let focused = self.focus == Focus::Side;
        let now = timefmt::now();
        if self.thread.is_some() {
            let open = self.thread.as_ref().map(|t| t.thread.clone());
            let rows: Vec<sidebar::Row> = self
                .rows
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let who = r.people.last().map_or("", |p| p.as_str());
                    let subj = if r.subject.is_empty() { "(no subject)" } else { r.subject.as_str() };
                    sidebar::Row {
                        text: format!("{}  {}", who, subj),
                        mark: Some(if r.unread { ('●', C_LINK) } else if r.flagged { ('★', C_STAR) } else { (' ', 0) }),
                        extra: timefmt::short_at(&r.date, now),
                        bold: r.unread,
                        dim: !r.unread,
                        current: open.as_deref() == Some(r.thread.as_str()),
                        target: Some(i),
                        ..sidebar::Row::default()
                    }
                })
                .collect();
            let sel = self.rows.iter().position(|r| open.as_deref() == Some(r.thread.as_str()));
            self.geo.side_map = rows.iter().map(|r| r.target).collect();
            self.geo.side = sidebar::draw(&mut self.screen, &rows, sel, &mut self.tside_st, (0, sw), (1, h), focused);
            return;
        }
        let cur = self.cur_box().map(String::from);
        let rows: Vec<sidebar::Row> = self
            .boxes
            .iter()
            .enumerate()
            .map(|(i, b)| sidebar::Row {
                depth: b.depth,
                text: b.name.clone(),
                extra: if b.unread > 0 { b.unread.to_string() } else { String::new() },
                extra_fg: C_LINK,
                bold: b.unread > 0,
                current: cur.as_deref() == Some(b.id.as_str()),
                target: Some(i),
                ..sidebar::Row::default()
            })
            .collect();
        self.geo.side_map = rows.iter().map(|r| r.target).collect();
        self.geo.side = sidebar::draw(&mut self.screen, &rows, Some(self.side_sel), &mut self.side_st, (0, sw), (1, h), focused);
    }

    fn render_list(&mut self, x0: usize, bottom: usize) {
        let w = self.screen.w;
        let (y0, rows) = (1, bottom.saturating_sub(1));
        let lw = w - x0;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        let mut top = self.geo.top;
        if self.sel < top {
            top = self.sel;
        }
        if self.sel >= top + rows {
            top = self.sel + 1 - rows;
        }
        top = top.min(self.rows.len().saturating_sub(rows));
        (self.geo.y0, self.geo.rows, self.geo.top, self.geo.lx0) = (y0, rows, top, x0);
        if self.rows.is_empty() {
            let s = if self.view.is_none() || !self.loaded { "loading…" } else if matches!(self.view, Some(View::Search(_))) { "no matches" } else { "nothing here" };
            self.screen.puts(x0 + 2, y0 + 1, s, Style::new(FG_DIM, 0, ITALIC), w);
            return;
        }
        let pw = (lw / 4).clamp(10, 24);
        let now = timefmt::now();
        let list_focus = self.focus == Focus::List || self.geo.side.w == 0;
        for (k, r) in self.rows.iter().enumerate().skip(top).take(rows) {
            let y = y0 + k - top;
            let sel = k == self.sel;
            let bg = if sel { BG_SEL } else { 0 };
            if sel {
                self.screen.fill(x0, w, y, Style::new(0, bg, 0));
                self.screen.put(x0, y, '▌', Style::new(if list_focus { ACCENT } else { FG_DIM }, bg, 0));
            }
            if self.marked.contains(&r.thread) {
                self.screen.put(x0 + 1, y, '✓', Style::new(C_OK, bg, BOLD));
            }
            if r.unread {
                self.screen.put(x0 + 2, y, '●', Style::new(C_LINK, bg, BOLD));
            }
            if r.flagged {
                self.screen.put(x0 + 3, y, '★', Style::new(C_STAR, bg, 0));
            }
            let bold = if r.unread { BOLD } else { 0 };
            let who = if r.people.is_empty() { "(no sender)".to_string() } else { r.people.join(", ") };
            let count = if r.emails.len() > 1 { format!(" {}", r.emails.len()) } else { String::new() };
            let cw = str_width(&count);
            let first = r.people.last().map_or("", |p| p.as_str());
            let px = x0 + 5;
            let ex = self.screen.puts(px, y, &who, Style::new(if r.unread { name_color(first) } else { 246 }, bg, bold), px + pw.saturating_sub(cw + 1));
            self.screen.puts(ex, y, &count, Style::new(FG_DIM, bg, 0), px + pw);
            let date = timefmt::short_at(&r.date, now);
            let dw = str_width(&date) + 1;
            let attach = if r.attach { " 📎" } else { "" };
            let right = format!("{}{}", attach, "");
            let rw = str_width(&right);
            let dx = w.saturating_sub(dw);
            self.screen.puts(dx, y, &date, Style::new(if r.unread { 252 } else { FG_DIM }, bg, bold), w);
            if rw > 0 {
                self.screen.puts(dx.saturating_sub(rw), y, &right, Style::new(FG_DIM, bg, 0), dx);
            }
            let sx = px + pw + 1;
            let lim = dx.saturating_sub(rw + 1);
            let subj = if r.subject.is_empty() { "(no subject)" } else { r.subject.as_str() };
            let x = self.screen.puts(sx, y, subj, Style::new(if r.unread { 255 } else { 250 }, bg, bold), lim);
            if x + 3 < lim && !r.preview.is_empty() {
                self.screen.puts(x, y, &format!(" – {}", r.preview), Style::new(FG_DIM, bg, 0), lim);
            }
        }
    }

    fn doc(&self, width: usize) -> (Vec<DL>, Vec<usize>) {
        let mut out: Vec<DL> = Vec::new();
        let mut starts = Vec::new();
        let Some(t) = &self.thread else { return (out, starts) };
        let mut uid = 0u32;
        let img_cols = width.saturating_sub(6).min(60);
        for (i, m) in t.msgs.iter().enumerate() {
            starts.push(out.len());
            if i > 0 {
                out.push(DL::new(vec![("─".repeat(width.saturating_sub(2)), Style::fg(238))], i));
            }
            let who = m.from.first().map_or("(unknown)".to_string(), |a| if m.expanded { a.full() } else { a.short().to_string() });
            let unread = m.mini.unread();
            let dim = Style::fg(FG_DIM);
            let mut head = vec![(if m.expanded { "▾ " } else { "▸ " }.to_string(), dim)];
            if unread {
                head.push(("● ".into(), Style::new(C_LINK, 0, BOLD)));
            }
            if m.mini.has("$flagged") {
                head.push(("★ ".into(), Style::fg(C_STAR)));
            }
            head.push((who, Style::new(m.from.first().map_or(250, |a| name_color(a.short())), 0, BOLD)));
            if m.mini.has("$draft") {
                head.push(("  [draft]".into(), Style::fg(C_WARN)));
            }
            if !m.expanded {
                let snippet = m.text.split_whitespace().filter(|w| !w.starts_with('>')).take(30).collect::<Vec<_>>().join(" ");
                head.push((format!("  {}", snippet), dim));
            }
            out.push(DL { right: timefmt::long(&m.sent), head: true, hit: Hit::Head, ..DL::new(head, i) });
            if !m.expanded {
                continue;
            }
            let addrs = |a: &[crate::model::Addr]| a.iter().map(|x| x.short().to_string()).collect::<Vec<_>>().join(", ");
            let mut to = format!("to {}", addrs(&m.to));
            if !m.cc.is_empty() {
                to += &format!("   cc {}", addrs(&m.cc));
            }
            for (a, b) in wrap(&to, width.saturating_sub(4).max(8)) {
                out.push(DL::new(vec![(format!("  {}", &to[a..b]), dim)], i));
            }
            let boxes: Vec<String> = m.mini.boxes.iter().map(|b| self.box_path(b)).collect();
            if boxes.len() > 1 || (boxes.len() == 1 && self.cur_box().is_none()) {
                out.push(DL::new(vec![(format!("  in {}", boxes.join(", ")), dim)], i));
            }
            for (k, a) in m.atts.iter().enumerate() {
                out.push(DL { hit: Hit::Att(k), ..DL::new(vec![(format!("  📎 {}  ({})  click to save", a.name, model::human_size(a.size)), Style::fg(C_LINK))], i) });
            }
            out.push(DL::new(Vec::new(), i));
            if m.text.trim().is_empty() {
                out.push(DL::new(vec![("  (no text)".into(), Style::new(FG_DIM, 0, ITALIC))], i));
            } else {
                let mut lines = Vec::new();
                body_lines(&m.text, width.saturating_sub(4), i, &mut lines);
                for mut l in lines {
                    l.spans.insert(0, ("  ".into(), Style::default()));
                    out.push(l);
                }
            }
            let remote_ok = self.remote_ok.contains(&m.mini.id);
            if self.gallery.enabled {
                for p in m.pics(remote_ok) {
                    out.push(DL::new(Vec::new(), i));
                    match self.gallery.size(&p.key, img_cols, 14) {
                        Some((cols, rows)) => {
                            uid += 1;
                            for r in 0..rows {
                                out.push(DL { img: Some((p.key.clone(), r, rows, cols, uid)), ..DL::new(Vec::new(), i) });
                            }
                        }
                        None => {
                            let note = match self.gallery.slot(&p.key) {
                                Some(mimg::Slot::Failed(e)) => format!(" ({})", e),
                                _ => " …".to_string(),
                            };
                            out.push(DL::new(vec![(format!("  🖼 {}{}", p.name, note), Style::fg(C_LINK))], i));
                        }
                    }
                }
                if !m.remote.is_empty() && !remote_ok {
                    out.push(DL::new(vec![(format!("  🖼 {} remote image{} not loaded  (I to load)", m.remote.len(), if m.remote.len() == 1 { "" } else { "s" }), Style::new(FG_DIM, 0, ITALIC))], i));
                }
            }
            if !m.links.is_empty() {
                out.push(DL { hit: Hit::Links, ..DL::new(vec![(format!("  {} link{}  (o to open)", m.links.len(), if m.links.len() == 1 { "" } else { "s" }), Style::new(FG_DIM, 0, ITALIC))], i) });
            }
            out.push(DL::new(Vec::new(), i));
        }
        (out, starts)
    }

    /// Ask for the images of the expanded messages that aren't here yet.
    fn fetch_images(&mut self) {
        if !self.gallery.enabled {
            return;
        }
        let Some(t) = &self.thread else { return };
        let mut want = Vec::new();
        for m in t.msgs.iter().filter(|m| m.expanded) {
            want.extend(m.pics(self.remote_ok.contains(&m.mini.id)));
        }
        for p in want {
            if self.gallery.request(&p.key) {
                self.pending += 1;
                match &p.att {
                    Some(a) => self.net.fetch_blob(&p.key, &a.blob, &a.name, &a.typ),
                    None => self.net.fetch_url(&p.key, &p.key),
                }
            }
        }
    }

    fn render_thread(&mut self, x0: usize, bottom: usize) {
        let w = self.screen.w;
        let view_h = bottom.saturating_sub(1);
        self.geo.trows.clear();
        if self.thread.as_ref().is_some_and(|t| t.loaded) {
            self.fetch_images();
        }
        let (doc, starts) = self.doc(w - x0);
        let Some(t) = &mut self.thread else { return };
        if !t.loaded {
            self.screen.puts(x0 + 2, 2, "loading…", Style::new(FG_DIM, 0, ITALIC), w);
            return;
        }
        t.starts = starts;
        if t.follow {
            t.follow = false;
            t.scroll = t.starts.get(t.cur).copied().unwrap_or(0);
        }
        t.scroll = t.scroll.min(doc.len().saturating_sub(view_h));
        let cur = t.cur;
        let scroll = t.scroll;
        let mut pics: Vec<(String, u32, usize, usize, usize, usize, usize)> = Vec::new(); // key, id, y, i0, i1, of, cols
        for (k, l) in doc.iter().enumerate().skip(scroll).take(view_h) {
            let y = 1 + k - scroll;
            self.geo.trows.push((l.msg, l.hit));
            let mark = l.msg == cur;
            let bg = if l.head && mark { BG_SEL } else { 0 };
            if l.head && mark {
                self.screen.fill(x0, w, y, Style::new(0, BG_SEL, 0));
            }
            if mark {
                self.screen.put(x0, y, '▌', Style::new(if l.head { ACCENT } else { 238 }, bg, 0));
            }
            if let Some((key, i, of, cols, id)) = &l.img {
                match pics.iter_mut().find(|p| p.1 == *id) {
                    Some(p) => p.4 = i + 1,
                    None => pics.push((key.clone(), *id, y, *i, i + 1, *of, *cols)),
                }
                continue;
            }
            let rw = str_width(&l.right);
            let lim = if rw > 0 { w.saturating_sub(rw + 2) } else { w };
            let mut x = x0 + 1;
            for (s, st) in &l.spans {
                x = self.screen.puts(x, y, s, Style { bg, ..*st }, lim);
            }
            if rw > 0 {
                self.screen.puts(w.saturating_sub(rw + 1), y, &l.right, Style::new(FG_DIM, bg, 0), w);
            }
        }
        if doc.len() > view_h {
            let s = format!("{}% ", (scroll + view_h) * 100 / doc.len());
            self.screen.puts(w.saturating_sub(s.len()), 0, &s, Style::new(FG_DIM, BG_BAR, 0), w);
        }
        if !matches!(self.mode, Mode::Pick(..) | Mode::Links(_) | Mode::Atts(_) | Mode::Help) {
            let ims = pics.iter().filter_map(|(key, id, y, i0, i1, of, cols)| self.gallery.place(key, *id, (x0 + 3, *y), *cols, *of, *i0, *i1)).collect();
            self.screen.set_images(ims);
        }
    }
}
