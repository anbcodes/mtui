use crate::model::*;
use mtui::lineedit::{Edit, LineEdit};
use mtui::screen::{Screen, Style, BOLD, REVERSE};
use mtui::term::Key;
use std::path::PathBuf;
use std::time::SystemTime;

const TEXT: u8 = 252;
const DIM: u8 = 244;
const ACCENT: u8 = 180;
const GOOD: u8 = 114;
const WARN: u8 = 209;
const BAD: u8 = 203;
const WHAT: u8 = 110;
const BAR: u8 = 236;
const SEL: u8 = 238;

const HELP: &[&str] = &[
    "j k  g G      move",
    "e Enter       enter a score (a digit starts it): 85  17/20  ~90  -",
    "              ~ marks a what-if: it counts in the projection only",
    "w             turn a score into a what-if and back",
    "x             clear the score",
    "o             add an item to this category: NAME [SCORE]",
    "r             rename the item",
    "d             delete the item",
    "a             what ungraded work is assumed to score (empty: your average)",
    "b             absences (when the course has an absence penalty)",
    "u             undo",
    "Tab           summary / list when the window is narrow",
    "E             edit the file in $EDITOR (mvi) for rules, weights, categories",
    "q             quit",
    "",
    "Every change is saved to the file at once.",
];

enum Prompt {
    Score(usize, usize),
    Rename(usize, usize),
    New(usize),
    Assume,
    Absent,
}

#[derive(Clone, Copy)]
enum Row {
    Cat(usize),
    Item(usize, usize),
}

pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub edit_file: bool,
    path: PathBuf,
    course: Course,
    undo: Vec<Course>,
    sel: usize,
    top: usize,
    prompt: Option<(Prompt, String, LineEdit)>,
    msg: String,
    help: bool,
    summary: bool,
    stamp: Option<SystemTime>,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl App {
    pub fn new(w: usize, h: usize, path: PathBuf, course: Course) -> App {
        let stamp = mtime(&path);
        let msg = course.warnings.first().cloned().unwrap_or_default();
        App { screen: Screen::new(w, h), quit: false, edit_file: false, path, course, undo: vec![], sel: 0, top: 0, prompt: None, msg, help: false, summary: false, stamp }
    }

    fn rows(&self) -> Vec<Row> {
        let mut v = vec![];
        for (i, c) in self.course.cats.iter().enumerate() {
            v.push(Row::Cat(i));
            // empty slots collapse into the first one: scoring it names it and the next shows up
            let mut seen_empty = false;
            for (j, it) in c.items.iter().enumerate() {
                if it.name.is_empty() && it.score == Score::None {
                    if seen_empty {
                        continue;
                    }
                    seen_empty = true;
                }
                v.push(Row::Item(i, j));
            }
        }
        v
    }

    fn cur(&self) -> Row {
        let r = self.rows();
        r[self.sel.min(r.len() - 1)]
    }

    pub fn tick(&mut self) {
        let m = mtime(&self.path);
        if m != self.stamp {
            self.stamp = m;
            match std::fs::read_to_string(&self.path).map_err(|e| e.to_string()).and_then(|s| parse(&s)) {
                Ok(c) => {
                    self.course = c;
                    self.msg = "file changed: reloaded".into();
                }
                Err(e) => self.msg = format!("file changed but not reloaded: {}", e),
            }
            self.sel = self.sel.min(self.rows().len() - 1);
        }
    }

    fn save(&mut self) {
        let tmp = self.path.with_extension("grades.tmp");
        let r = std::fs::write(&tmp, self.course.serialize()).and_then(|_| std::fs::rename(&tmp, &self.path));
        if let Err(e) = r {
            self.msg = format!("cannot save: {}", e);
        }
        self.stamp = mtime(&self.path);
    }

    fn change(&mut self, f: impl FnOnce(&mut Course)) {
        self.undo.push(self.course.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        f(&mut self.course);
        self.save();
    }

    pub fn run_editor(&mut self) {
        let ed = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "mvi".into());
        mtui::term::disable_raw();
        let st = std::process::Command::new("sh").arg("-c").arg(format!("{} \"$0\"", ed)).arg(&self.path).status();
        let _ = mtui::term::enable_raw();
        self.screen.invalidate();
        if let Err(e) = st {
            self.msg = format!("{}: {}", ed, e);
        }
        self.stamp = None; // reload whatever the editor wrote
    }

    // ---- keys ----

    pub fn handle_key(&mut self, k: Key) {
        self.msg.clear();
        if self.prompt.is_some() {
            return self.prompt_key(k);
        }
        if self.help {
            self.help = false;
            return;
        }
        let n = self.rows().len();
        let page = self.screen.h.saturating_sub(4).max(1);
        match k {
            Key::Char('q') | Key::Ctrl('c') => self.quit = true,
            Key::Char('j') | Key::Down => self.sel = (self.sel + 1).min(n - 1),
            Key::Char('k') | Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Char('g') | Key::Home => self.sel = 0,
            Key::Char('G') | Key::End => self.sel = n - 1,
            Key::PageDown | Key::Ctrl('d') => self.sel = (self.sel + page / 2).min(n - 1),
            Key::PageUp | Key::Ctrl('u') => self.sel = self.sel.saturating_sub(page / 2),
            Key::Char('?') => self.help = true,
            Key::Tab => self.summary = !self.summary,
            Key::Char('E') => self.edit_file = true,
            Key::Char('u') => match self.undo.pop() {
                Some(c) => {
                    self.course = c;
                    self.sel = self.sel.min(self.rows().len() - 1);
                    self.save();
                    self.msg = "undone".into();
                }
                None => self.msg = "nothing to undo".into(),
            },
            Key::Char('a') => {
                let t = self.course.assume.map_or(String::new(), num);
                self.ask(Prompt::Assume, "Assume ungraded work scores (empty = your average so far): ", &t);
            }
            Key::Char('b') => {
                let t = self.course.absent.to_string();
                self.ask(Prompt::Absent, "Absences: ", &t);
            }
            Key::Char('o') => {
                let c = match self.cur() {
                    Row::Cat(c) | Row::Item(c, _) => c,
                };
                self.ask(Prompt::New(c), "New item, NAME [SCORE]: ", "");
            }
            _ => {
                if let Row::Item(c, i) = self.cur() {
                    self.item_key(k, c, i);
                }
            }
        }
    }

    fn item_key(&mut self, k: Key, c: usize, i: usize) {
        let item = &self.course.cats[c].items[i];
        let label = format!("Score for {} (85, 17/20, ~90 what-if, - clear): ", self.item_name(c, i));
        match k {
            Key::Enter | Key::Char('e') => {
                let t = if item.score == Score::None { String::new() } else { item.score.show() };
                self.ask(Prompt::Score(c, i), &label, &t);
            }
            Key::Char(ch) if ch.is_ascii_digit() || ch == '~' => self.ask(Prompt::Score(c, i), &label, &ch.to_string()),
            Key::Char('x') | Key::Delete => self.change(|co| co.cats[c].items[i].score = Score::None),
            Key::Char('w') => match item.score {
                Score::Got(e, m) => self.change(|co| co.cats[c].items[i].score = Score::What(e, m)),
                Score::What(e, m) => self.change(|co| co.cats[c].items[i].score = Score::Got(e, m)),
                Score::None => self.msg = "no score to turn into a what-if; type ~90".into(),
            },
            Key::Char('r') => {
                let t = item.name.clone();
                self.ask(Prompt::Rename(c, i), "Name: ", &t);
            }
            Key::Char('d') => {
                self.change(|co| {
                    co.cats[c].items.remove(i);
                });
                self.sel = self.sel.min(self.rows().len() - 1);
                self.msg = "deleted (u undoes)".into();
            }
            _ => {}
        }
    }

    fn item_name(&self, c: usize, i: usize) -> String {
        let n = &self.course.cats[c].items[i].name;
        if n.is_empty() { format!("{} #{}", self.course.cats[c].name, i + 1) } else { n.clone() }
    }

    fn ask(&mut self, p: Prompt, label: &str, text: &str) {
        let mut e = LineEdit::new();
        e.set(text);
        self.prompt = Some((p, label.to_string(), e));
    }

    fn prompt_key(&mut self, k: Key) {
        match k {
            Key::Esc | Key::Ctrl('c') => self.prompt = None,
            Key::Enter => {
                let Some((p, _, e)) = self.prompt.take() else { return };
                self.commit(p, e.text.trim().to_string());
            }
            k => {
                if let Some((_, _, e)) = &mut self.prompt {
                    if let Edit::Ignored = e.key(&k) {}
                }
            }
        }
    }

    fn commit(&mut self, p: Prompt, text: String) {
        match p {
            Prompt::Score(c, i) => match Score::parse(&text) {
                Some(s) => {
                    self.change(|co| {
                        let it = &mut co.cats[c].items[i];
                        it.score = s;
                        if it.name.is_empty() && s != Score::None {
                            it.name = format!("#{}", i + 1);
                        }
                    });
                    // on to the next item, to type a run of scores quickly
                    let rows = self.rows();
                    if let Some(n) = rows.iter().skip(self.sel + 1).position(|r| matches!(r, Row::Item(..))) {
                        self.sel += 1 + n;
                    }
                }
                None => self.msg = "not a score: try 85, 17/20, ~90 or -".into(),
            },
            Prompt::Rename(c, i) => {
                if !text.is_empty() {
                    self.change(|co| co.cats[c].items[i].name = text);
                }
            }
            Prompt::New(c) => {
                if text.is_empty() {
                    return;
                }
                let toks: Vec<&str> = text.split_whitespace().collect();
                let (name, score) = match Score::parse(toks[toks.len() - 1]) {
                    Some(s) if toks.len() > 1 => (toks[..toks.len() - 1].join(" "), s),
                    _ => (text.clone(), Score::None),
                };
                self.change(|co| co.cats[c].items.push(Item::new(&name, score)));
                let at = self.rows().iter().position(|r| matches!(r, Row::Item(cc, j) if *cc == c && *j + 1 == self.course.cats[c].items.len()));
                if let Some(at) = at {
                    self.sel = at;
                }
            }
            Prompt::Assume => {
                if text.is_empty() {
                    self.change(|co| co.assume = None);
                } else if let Ok(v) = text.trim_end_matches('%').parse::<f64>() {
                    self.change(|co| co.assume = Some(v));
                } else {
                    self.msg = "not a number".into();
                }
            }
            Prompt::Absent => match text.parse::<usize>() {
                Ok(v) => {
                    self.change(|co| co.absent = v);
                    if self.course.absence_rule.is_none() {
                        self.msg = "this course has no absence-penalty line (E to add one)".into();
                    }
                }
                Err(_) => self.msg = "not a number".into(),
            },
        }
    }

    // ---- drawing ----

    pub fn render(&mut self) {
        let (w, h) = (self.screen.w, self.screen.h);
        if w < 20 || h < 6 {
            return;
        }
        self.screen.clear();
        let bar = Style::new(TEXT, BAR, 0);
        let cur = self.course.eval(Mode::Current);
        let fill = self.course.fill();
        let proj = self.course.eval(Mode::Proj(fill));
        self.screen.fill(0, w, 0, bar);
        let title = if self.course.title.is_empty() { self.path.display().to_string() } else { self.course.title.clone() };
        self.screen.puts(1, 0, &title, Style::new(ACCENT, BAR, BOLD), w);
        let mut right = String::new();
        if let (Some(a), Some(r)) = (cur.avg, cur.rank) {
            right = format!("{} {:.1}%", self.course.letter(r), a);
            if let (Some(pa), Some(pr)) = (proj.avg, proj.rank) {
                if self.course.pending() > 0 {
                    right.push_str(&format!("  ->  {} {:.1}%", self.course.letter(pr), pa));
                }
            }
        }
        let rw = right.chars().count();
        self.screen.puts(w.saturating_sub(rw + 2), 0, &right, Style::new(GOOD, BAR, BOLD), w);

        let wide = w >= 110;
        let lw = if wide { w - 59 } else { w };
        let body_h = h - 3;
        if self.help {
            for (i, l) in HELP.iter().enumerate().take(body_h) {
                self.screen.puts(2, 1 + i, l, Style::fg(TEXT), w);
            }
        } else if wide || !self.summary {
            self.draw_list(lw, body_h, &cur);
        }
        if (wide || self.summary) && !self.help {
            let x = if wide { lw + 2 } else { 1 };
            if wide {
                for y in 1..h - 1 {
                    self.screen.put(lw, y, '│', Style::fg(DIM));
                }
            }
            let mut y = 1;
            for (tone, l) in self.course.report() {
                if y >= h - 1 {
                    break;
                }
                let st = match tone {
                    Tone::Head => Style::new(TEXT, 0, BOLD),
                    Tone::Good => Style::fg(GOOD),
                    Tone::Warn => Style::fg(WARN),
                    Tone::Bad => Style::fg(BAD),
                    Tone::Dim => Style::fg(DIM),
                    Tone::Plain => Style::fg(TEXT),
                };
                self.screen.puts(x, y, &l, st, w);
                y += 1;
            }
            for wn in &self.course.warnings {
                if y >= h - 1 {
                    break;
                }
                y += 1;
                self.screen.puts(x, y - 1, &format!("! {}", wn), Style::fg(WARN), w);
            }
        }

        // bottom line
        self.screen.fill(0, w, h - 1, bar);
        let mut cursor = None;
        if let Some((_, label, e)) = &self.prompt {
            let lx = self.screen.puts(1, h - 1, label, Style::new(ACCENT, BAR, 0), w);
            let (cx, cy) = e.draw(&mut self.screen, lx, h - 1, w.saturating_sub(lx + 1), 1, Style::new(TEXT, BAR, 0));
            cursor = Some((cx, cy, true));
        } else if !self.msg.is_empty() {
            self.screen.puts(1, h - 1, &self.msg, Style::new(WARN, BAR, 0), w);
        } else {
            self.screen.puts(1, h - 1, "e score  o add  w what-if  a assume  u undo  E edit file  ? keys  q quit", Style::new(DIM, BAR, 0), w);
        }
        self.screen.flush(cursor);
    }

    fn draw_list(&mut self, lw: usize, body_h: usize, cur: &Eval) {
        let tw: f64 = self.course.cats.iter().map(|c| c.weight).sum();
        let rows = self.rows();
        self.sel = self.sel.min(rows.len() - 1);
        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + body_h - 1 {
            self.top = self.sel + 2 - body_h;
        }
        let cw = 24usize; // right-hand columns
        let nx = lw.saturating_sub(cw);
        let head = Style::fg(DIM);
        self.screen.puts(2, 1, "category / item", head, nx);
        self.screen.puts(nx, 1, &format!("{:>6} {:>8} {:>8}", "weight", "average", "points"), head, lw);
        for (n, row) in rows.iter().enumerate().skip(self.top).take(body_h - 1) {
            let y = 2 + n - self.top;
            let sel = n == self.sel;
            let bg = if sel { SEL } else { 0 };
            self.screen.fill(0, lw, y, Style::new(TEXT, bg, 0));
            match *row {
                Row::Cat(c) => {
                    let cat = &self.course.cats[c];
                    let calc = &cur.cats[c];
                    let avg = calc.avg.map_or("-".to_string(), |a| format!("{:.2}", a));
                    let pts = calc.avg.map_or("-".to_string(), |a| format!("{:.2}", a * cat.weight / tw.max(1e-9)));
                    let mut tag = String::new();
                    if cat.drop > 0 {
                        tag.push_str(&format!("  drop {}", cat.drop));
                    }
                    if let Some(g) = &cat.group {
                        tag.push_str(&format!("  [{}]", g));
                    }
                    let x = self.screen.puts(1, y, &cat.name, Style::new(ACCENT, bg, BOLD), nx);
                    self.screen.puts(x, y, &tag, Style::new(DIM, bg, 0), nx);
                    self.screen.puts(nx, y, &format!("{:>5}% {:>8} {:>8}", num(cat.weight), avg, pts), Style::new(TEXT, bg, 0), lw);
                }
                Row::Item(c, i) => {
                    let cat = &self.course.cats[c];
                    let it = &cat.items[i];
                    let dropped = cur.cats[c].dropped.contains(&i);
                    let name = if it.name.is_empty() { format!("#{}", i + 1) } else { it.name.clone() };
                    let (fg, attr) = match it.score {
                        Score::None => (DIM, 0),
                        Score::What(..) => (WHAT, 0),
                        Score::Got(..) if dropped => (DIM, 0),
                        Score::Got(..) => (TEXT, 0),
                    };
                    let x = self.screen.puts(4, y, &name, Style::new(if it.name.is_empty() { DIM } else { fg }, bg, attr), nx);
                    if dropped {
                        self.screen.puts(x + 1, y, "(dropped)", Style::new(DIM, bg, 0), nx);
                    } else if it.name.is_empty() && it.score == Score::None {
                        let more = cat.items.iter().filter(|j| j.name.is_empty() && j.score == Score::None).count();
                        if more > 1 {
                            self.screen.puts(x + 1, y, &format!("(+{} more empty)", more - 1), Style::new(DIM, bg, 0), nx);
                        }
                    }
                    let pct = it.score.pct().map_or(String::new(), |p| format!("{:.1}%", p));
                    self.screen.puts(nx + 7, y, &format!("{:>8} {:>8}", it.score.show(), pct), Style::new(fg, bg, attr), lw);
                }
            }
            if sel {
                self.screen.set_style(0, y, Style::new(ACCENT, bg, REVERSE));
            }
        }
    }
}
