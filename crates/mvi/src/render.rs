// Drawing the editor into the cell grid.

use crate::diag::Sev;
use crate::editor::{disp_col, Editor, Mode, TK};
use mtui::screen::{char_width, Style, BOLD, ITALIC, UNDERLINE};
use crate::syntax as sx;

const FG_DIM: u8 = 246;
const BG_CURLINE: u8 = 235;
const BG_SEL: u8 = 238;
const BG_SEARCH: u8 = 58;
const BG_STATUS: u8 = 236;
const BG_POPUP: u8 = 237;
const BG_POPSEL: u8 = 24;
const C_ERR: u8 = 203;
const C_WARN: u8 = 214;
const C_INFO: u8 = 110;

fn kind_style(k: u8) -> Style {
    sx::style(k)
}

fn sev_color(s: Sev) -> u8 {
    match s {
        Sev::Error => C_ERR,
        Sev::Warning => C_WARN,
        Sev::Info => C_INFO,
    }
}

impl Editor {
    pub fn gutter_w(&self) -> usize {
        let b = self.bb();
        let digits = b.lines.len().to_string().len().max(3);
        if self.opts.number || self.opts.relnum {
            digits + 2
        } else {
            2
        }
    }

    pub fn scroll(&mut self) {
        let th = self.text_h();
        let tw = self.screen.w.saturating_sub(self.gutter_w()).max(1);
        let ts = self.opts.tabstop;
        let b = self.b();
        let so = 3.min(th.saturating_sub(1) / 2);
        if b.cy < b.top + so {
            b.top = b.cy.saturating_sub(so);
        }
        let so_b = so.min(b.lines.len() - 1 - b.cy);
        if b.cy + so_b >= b.top + th {
            b.top = (b.cy + so_b + 1).saturating_sub(th);
        }
        b.top = b.top.min(b.lines.len().saturating_sub(1));
        let dc = disp_col(&b.lines[b.cy], b.cx, ts);
        if dc < b.left {
            b.left = dc.saturating_sub(4);
        }
        if dc >= b.left + tw {
            b.left = dc + 8 - tw.min(dc + 8);
        }
    }

    /// Draw a frame to the terminal.
    pub fn render(&mut self) {
        let cursor = self.draw();
        self.screen.flush(cursor);
    }

    /// Draw a frame into `self.screen` without sending it; returns where the
    /// cursor goes (x, y, bar shape).
    pub fn draw(&mut self) -> Option<(usize, usize, bool)> {
        if self.preview.is_some() {
            return self.draw_preview();
        }
        self.scroll();
        let (w, h) = (self.screen.w, self.screen.h);
        if w < 10 || h < 3 {
            return None;
        }
        self.screen.clear();
        let th = self.text_h();
        let gw = self.gutter_w();
        let ts = self.opts.tabstop;
        let mode = self.mode;
        let vis = matches!(mode, Mode::Visual | Mode::VisualLine).then(|| self.vis_range());
        let (cy, cx) = self.cursor();
        let pair = if matches!(mode, Mode::Normal | Mode::Insert) { self.match_pair((cy, cx)).map(|p| (p, (cy, cx))) } else { None };
        let show_search = self.show_hl && self.opts.hlsearch && self.search.is_some();
        let mut hl = std::mem::take(&mut self.hl_scratch);
        let mut matches: Vec<(usize, usize)> = Vec::new();
        let b = &mut self.bufs[self.cur];
        let (top, left) = (b.top, b.left);
        let lang = b.lang;
        for row in 0..th {
            let ln = top + row;
            if ln >= b.lines.len() {
                self.screen.put(0, row, '~', Style::fg(241));
                continue;
            }
            // gutter
            let worst = b.diags.iter().filter(|d| d.line == ln).map(|d| d.sev).min();
            if let Some(s) = worst {
                self.screen.put(0, row, match s { Sev::Error => 'E', Sev::Warning => 'W', Sev::Info => 'I' }, Style::new(sev_color(s), 0, BOLD));
            }
            if self.opts.number || self.opts.relnum {
                let n = if self.opts.relnum && ln != cy { (ln as isize - cy as isize).unsigned_abs() } else { ln + 1 };
                let s = format!("{:>width$} ", n, width = gw - 2);
                let st = if ln == cy { Style::new(250, 0, BOLD) } else { Style::fg(243) };
                self.screen.puts(1, row, &s, st, gw);
            }
            // text
            let line = &b.lines[ln];
            if line.len() > 20000 {
                // skip highlighting pathological lines
                hl.clear();
                hl.resize(line.len(), 0);
            } else {
                b.hl.line(lang, &b.lines, ln, &mut hl);
            }
            let line = &b.lines[ln];
            matches.clear();
            if show_search {
                let re = self.search.as_ref().unwrap();
                let mut p = 0;
                while p <= line.len() && matches.len() < 200 {
                    match re.find_at(line, p) {
                        Some((s, e)) => {
                            matches.push((s, e.max(s + 1)));
                            p = if e > s { e } else { s + 1 };
                        }
                        None => break,
                    }
                }
            }
            let cur_line = ln == cy && mode != Mode::VisualLine;
            let base_bg = if cur_line { BG_CURLINE } else { 0 };
            let mut dc = 0usize;
            let max_x = w;
            for (i, c) in line.char_indices() {
                let cw = if c == '\t' { ts - dc % ts } else { char_width(c) };
                if dc + cw > left + (w - gw) {
                    break;
                }
                if dc >= left {
                    let mut st = kind_style(hl.get(i).copied().unwrap_or(0));
                    st.bg = base_bg;
                    if let Some(t) = &vis {
                        let inside = match t.kind {
                            TK::Line => ln >= t.a.0 && ln <= t.b.0,
                            _ => (ln, i) >= t.a && (ln, i) <= t.b,
                        };
                        if inside {
                            st.bg = BG_SEL;
                        }
                    }
                    if matches.iter().any(|&(s, e)| i >= s && i < e) {
                        st.bg = BG_SEARCH;
                        st.fg = 230;
                    }
                    if let Some((p, q)) = pair {
                        if (ln, i) == p || (ln, i) == q {
                            st.bg = 240;
                            st.attr |= BOLD;
                        }
                    }
                    let x = gw + dc - left;
                    if c == '\t' {
                        for k in 0..cw {
                            let ch = if self.opts.list && k == 0 { '›' } else { ' ' };
                            let mut s2 = st;
                            if self.opts.list {
                                s2.fg = 238;
                            }
                            self.screen.put(x + k, row, ch, s2);
                        }
                    } else {
                        self.screen.put(x, row, c, st);
                    }
                }
                dc += cw;
            }
            // diagnostics: underline the span and add end-of-line text
            let mut eol_x = (gw + dc.saturating_sub(left)).max(gw);
            if dc < left {
                eol_x = gw;
            }
            if cur_line {
                self.screen.fill(eol_x, max_x, row, Style::new(0, BG_CURLINE, 0));
            }
            if let Some(t) = &vis {
                // show selected newline
                let inside = match t.kind {
                    TK::Line => ln >= t.a.0 && ln <= t.b.0,
                    _ => ln >= t.a.0 && ln < t.b.0 || (ln == t.b.0 && t.b.1 >= line.len() && ln >= t.a.0),
                };
                if inside && eol_x < w {
                    self.screen.set_style(eol_x, row, Style::new(0, BG_SEL, 0));
                }
            }
            let mut best: Option<&crate::diag::Diag> = None;
            for d in b.diags.iter().filter(|d| d.line == ln) {
                let s = line.char_indices().nth(d.col).map_or(line.len(), |(i, _)| i);
                let mut e = s;
                let bytes = line.as_bytes();
                while e < line.len() && (bytes[e].is_ascii_alphanumeric() || bytes[e] == b'_' || bytes[e] >= 0x80) {
                    e += 1;
                }
                if e == s {
                    e = (s + 1).min(line.len());
                }
                let (ds, de) = (disp_col(line, s, ts), disp_col(line, e, ts));
                for xx in ds.max(left)..de {
                    let x = gw + xx - left;
                    if x < w {
                        let mut st = self.screen.get_style(x, row);
                        st.attr |= UNDERLINE;
                        st.fg = sev_color(d.sev);
                        self.screen.set_style(x, row, st);
                    }
                }
                if best.map_or(true, |bd| d.sev < bd.sev) {
                    best = Some(d);
                }
            }
            if let Some(d) = best {
                let x = eol_x + 3;
                if x + 4 < w {
                    let first = d.msg.lines().next().unwrap_or("");
                    let msg = format!("■ {}", first);
                    self.screen.puts(x, row, &msg, Style::new(sev_color(d.sev), base_bg, ITALIC), w);
                }
            }
        }
        self.hl_scratch = hl;

        // status line
        let sy = h - 2;
        let b = &self.bufs[self.cur];
        let (bname, bdirty, blang) = (b.name.clone(), b.dirty, b.lang.name);
        let ne = b.diags.iter().filter(|d| d.sev == Sev::Error).count();
        let nw = b.diags.iter().filter(|d| d.sev == Sev::Warning).count();
        let col = disp_col(&b.lines[cy], cx, ts) + 1;
        let pct = if b.lines.len() <= 1 { 100 } else { cy * 100 / (b.lines.len() - 1) };
        let (mname, mbg) = match mode {
            Mode::Normal => ("NORMAL", 110),
            Mode::Insert => ("INSERT", 114),
            Mode::Visual => ("VISUAL", 176),
            Mode::VisualLine => ("V-LINE", 176),
            Mode::Cmd(_) => ("COMMAND", 180),
        };
        let mname = if self.pending.is_empty() || mode != Mode::Normal { mname.to_string() } else { "NORMAL".to_string() };
        self.screen.fill(0, w, sy, Style::new(252, BG_STATUS, 0));
        let mut x = self.screen.puts(0, sy, &format!(" {} ", mname), Style::new(16, mbg, BOLD), w);
        x = self.screen.puts(x + 1, sy, &bname, Style::new(252, BG_STATUS, BOLD), w);
        if bdirty {
            x = self.screen.puts(x, sy, " [+]", Style::new(C_WARN, BG_STATUS, BOLD), w);
        }
        if self.bufs.len() > 1 {
            self.screen.puts(x + 1, sy, &format!("({}/{})", self.cur + 1, self.bufs.len()), Style::new(FG_DIM, BG_STATUS, 0), w);
        }
        let mut right: Vec<(String, Style)> = Vec::new();
        if let Some(r) = self.recording_reg() {
            right.push((format!("rec @{} ", r), Style::new(C_ERR, BG_STATUS, BOLD)));
        }
        if !self.pending.is_empty() {
            let p: String = self.pending.iter().map(|k| match k {
                mtui::term::Key::Char(c) => *c,
                _ => '·',
            }).collect();
            right.push((format!("{} ", p), Style::new(180, BG_STATUS, 0)));
        }
        if self.check_running {
            right.push(("checking… ".into(), Style::new(C_INFO, BG_STATUS, 0)));
        }
        if ne > 0 {
            right.push((format!("E{} ", ne), Style::new(C_ERR, BG_STATUS, BOLD)));
        }
        if nw > 0 {
            right.push((format!("W{} ", nw), Style::new(C_WARN, BG_STATUS, BOLD)));
        }
        right.push((format!("{} ", blang), Style::new(FG_DIM, BG_STATUS, 0)));
        right.push((format!(" {}:{} {:>3}% ", cy + 1, col, pct), Style::new(16, mbg, 0)));
        let rw: usize = right.iter().map(|(s, _)| s.chars().count()).sum();
        let mut rx = w.saturating_sub(rw);
        for (s, st) in right {
            rx = self.screen.puts(rx, sy, &s, st, w);
        }

        // message / command line
        let my = h - 1;
        let mut cursor = None;
        if let Mode::Cmd(c) = mode {
            let s = format!("{}{}", c, self.cmdline);
            let total: usize = s.chars().map(char_width).sum();
            let skip = total.saturating_sub(w - 1);
            let shown: String = s.chars().skip(skip).collect();
            let x = self.screen.puts(0, my, &shown, Style::default(), w);
            cursor = Some((x, my, true));
        } else if !self.msg.is_empty() {
            let st = if self.msg_err { Style::new(C_ERR, 0, BOLD) } else { Style::default() };
            let m = self.msg.lines().next().unwrap_or("").to_string();
            self.screen.puts(0, my, &m, st, w);
        } else if let Some(d) = self.bb().diags.iter().filter(|d| d.line == cy).min_by_key(|d| d.sev) {
            let m = format!("{:?}: {}", d.sev, d.msg.lines().next().unwrap_or(""));
            self.screen.puts(0, my, &m, Style::fg(sev_color(d.sev)), w);
        }

        // cursor position in text
        let b = self.bb();
        let cdc = disp_col(&b.lines[cy], cx, ts);
        let scr_x = gw + cdc.saturating_sub(b.left);
        let scr_y = cy.saturating_sub(b.top);
        if cursor.is_none() {
            cursor = Some((scr_x.min(w - 1), scr_y, mode == Mode::Insert));
        }

        // completion popup
        if let Some(c) = &self.comp {
            if mode == Mode::Insert && c.line == cy {
                let n = c.items.len().min(10);
                let sel = c.sel.unwrap_or(0);
                let first = if sel >= n { sel + 1 - n } else { 0 };
                let lw = c.items.iter().map(|i| i.text.chars().count()).max().unwrap_or(1).min(40);
                let pw = lw + 4;
                let sx = (gw + disp_col(&b.lines[cy], c.start, ts)).saturating_sub(b.left + 1).min(w.saturating_sub(pw));
                let detail = c.items.get(sel).map(|i| i.detail.clone()).unwrap_or_default();
                let rows = n + if detail.is_empty() { 0 } else { 1 };
                let below = scr_y + 1 + rows <= th;
                let y0 = if below { scr_y + 1 } else { scr_y.saturating_sub(rows) };
                for k in 0..n {
                    let it = &c.items[first + k];
                    let selected = c.sel == Some(first + k);
                    let bg = if selected { BG_POPSEL } else { BG_POPUP };
                    let y = y0 + k;
                    self.screen.fill(sx, sx + pw, y, Style::new(0, bg, 0));
                    let tag_c = match it.kind {
                        crate::complete::Kind::Symbol => 75,
                        crate::complete::Kind::Keyword => 176,
                        crate::complete::Kind::Path => 180,
                        _ => FG_DIM,
                    };
                    self.screen.puts(sx + 1, y, it.kind.tag(), Style::new(tag_c, bg, 0), sx + pw);
                    self.screen.puts(sx + 3, y, &it.text, Style::new(if selected { 255 } else { 252 }, bg, if selected { BOLD } else { 0 }), sx + pw);
                }
                if !detail.is_empty() {
                    let y = y0 + n;
                    let dw = (detail.chars().count() + 2).min(w - sx);
                    self.screen.fill(sx, sx + dw, y, Style::new(0, 234, 0));
                    self.screen.puts(sx + 1, y, &detail, Style::new(250, 234, ITALIC), sx + dw);
                }
            }
        }

        // multi-line message overlay
        if !self.msg_lines.is_empty() {
            let n = self.msg_lines.len().min(h - 2);
            let y0 = h - 1 - n - 1;
            for k in 0..n {
                let y = y0 + k;
                self.screen.fill(0, w, y, Style::new(0, 234, 0));
                let l = self.msg_lines[k].replace('\t', "    ");
                self.screen.puts(1, y, &l, Style::new(252, 234, 0), w);
            }
            let more = if self.msg_lines.len() > n { format!("-- {} more lines hidden --  ", self.msg_lines.len() - n) } else { String::new() };
            self.screen.fill(0, w, h - 2, Style::new(0, 234, 0));
            self.screen.puts(1, h - 2, &format!("{}press any key", more), Style::new(FG_DIM, 234, ITALIC), w);
            cursor = None;
        }

        // picker overlay
        if let Some(p) = &mut self.picker {
            let (x, y) = p.draw(&mut self.screen, h - 2);
            cursor = Some((x, y, true));
        }

        cursor
    }
}
