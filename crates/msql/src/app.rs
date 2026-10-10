// The msql screen: a schema tree on the left, an mvi editor above and the
// results below. Queries run on a background thread and report back through
// a channel that wakes the event loop.

use crate::backend::{Backend, Kind, Output, ResultSet};
use crate::config;
use crate::schema::Schema;
use crate::sql;
use mtui::picker::{Pick, Picker};
use mtui::screen::{char_width, Screen};
use mtui::term::{Key, MouseKind, Waker};
use mvi::Pane;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

pub const HELP: &str = "\
msql: write SQL in mvi, run it on the database, browse the results.

Running        Alt-Enter or Ctrl-G or :run   the statement under the cursor (or the visual selection)
               :runall                        every statement in the buffer
Completion     pops up as you type: tables after FROM/JOIN, `alias.` columns, columns of the
               tables the statement reads, SQLite functions and keywords. Ctrl-Space opens it
               by hand; Tab / Ctrl-N / Ctrl-P select, Enter accepts
Moving         Alt-h schema   Alt-j results   Alt-k editor   Alt-l editor   (or Ctrl-W h/j/k/l)
Queries        :w saves the file you are in; :save NAME keeps this query under a name;
               :open [NAME] picks a saved query; :new [NAME]; :rm NAME
Schema         :schema shows or hides the tree; :refresh reloads it from the database
Results        :export FILE.csv|tsv|json writes the whole result
Schema tree    j k move   Enter open/close a table   i insert the name   p preview 100 rows   r refresh
Results        h j k l move   g G 0 $ ends   Ctrl-D Ctrl-U page   Enter show the row   y copy the cell
               Y copy the row (tab separated)   : command   Esc back to the editor
Row view       j k scroll   n p next/previous row   y copy   Esc back
:q quits.  This help closes on any key.";

pub enum Event {
    Query(u64, Result<Output, String>, Duration),
    Schema(Result<Schema, String>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Editor,
    Results,
    Schema,
}

struct Grid {
    set: ResultSet,
    widths: Vec<usize>,
    numeric: Vec<bool>,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
}

enum Body {
    Empty,
    Table(Box<Grid>),
    Message(String),
    Error(String),
}

struct Record {
    scroll: usize,
}

pub struct App {
    pub screen: Screen,
    backend: Arc<dyn Backend>,
    schema: Arc<RwLock<Schema>>,
    schema_state: Result<Option<usize>, String>, // Ok(None) = loading
    pane: Pane,
    path: PathBuf,
    qdir: PathBuf,
    state_dir: PathBuf,
    focus: Focus,
    win: bool,
    show_schema: bool,
    open: HashSet<String>,
    tree_sel: usize,
    tree_top: usize,
    body: Body,
    info: String,
    record: Option<Record>,
    running: Option<(u64, Instant)>,
    next_id: u64,
    note: Option<(String, bool)>,
    help: bool,
    ask: Option<String>,
    picker: Option<Picker<String>>,
    tx: Sender<Event>,
    waker: Waker,
    pub quit: bool,
    pub suspend: bool,
}

const HOST_CMDS: &[&str] = &["run", "runall", "save", "open", "new", "rm", "schema", "refresh", "export", "msql"];

const BG_BAR: u8 = 236;
const BG_SEL: u8 = 24;
const BG_ROW: u8 = 237;
const FG_DIM: u8 = 246;
const C_OK: u8 = 114;
const C_BAD: u8 = 203;
const C_HEAD: u8 = 75;
const C_NUM: u8 = 180;
const ACCENT: u8 = 180;

fn clip(s: &str, w: usize) -> String {
    let s: String = s.chars().map(|c| if c == '\n' { '⏎' } else if c == '\t' { ' ' } else if c.is_control() { '·' } else { c }).collect();
    let mut out = String::new();
    let mut used = 0;
    let total: usize = s.chars().map(char_width).sum();
    for c in s.chars() {
        let cw = char_width(c);
        if total > w && used + cw + 1 > w {
            out.push('…');
            return out;
        }
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out
}

fn disp_w(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    for l in text.split('\n') {
        let mut cur = String::new();
        let mut used = 0;
        for c in l.chars() {
            let cw = char_width(c);
            if used + cw > w.max(1) {
                out.push(std::mem::take(&mut cur));
                used = 0;
            }
            cur.push(if c == '\t' { ' ' } else { c });
            used += cw;
        }
        out.push(cur);
    }
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl Grid {
    fn new(set: ResultSet) -> Grid {
        let n = set.columns.len();
        let mut widths: Vec<usize> = set.columns.iter().map(|c| disp_w(c)).collect();
        let mut numeric = vec![true; n];
        for r in set.rows.iter().take(1000) {
            for (i, v) in r.iter().enumerate().take(n) {
                widths[i] = widths[i].max(disp_w(&v.text.replace('\n', "⏎")));
                if v.kind == Kind::Text {
                    numeric[i] = false;
                }
            }
        }
        for w in &mut widths {
            *w = (*w).clamp(1, 48);
        }
        Grid { set, widths, numeric, row: 0, col: 0, top: 0, left: 0 }
    }

    fn cell(&self) -> &str {
        self.set.rows.get(self.row).and_then(|r| r.get(self.col)).map_or("", |v| v.text.as_str())
    }

    fn row_tsv(&self, row: usize) -> String {
        self.set.rows.get(row).map(|r| r.iter().map(|v| v.text.replace(['\t', '\n'], " ")).collect::<Vec<_>>().join("\t")).unwrap_or_default()
    }
}

impl App {
    pub fn new(w: usize, h: usize, backend: Arc<dyn Backend>, tx: Sender<Event>, waker: Waker) -> App {
        let state_dir = config::data_dir().join(backend.key());
        let qdir = state_dir.join("queries");
        let _ = std::fs::create_dir_all(&qdir);
        let schema = Arc::new(RwLock::new(std::fs::read_to_string(state_dir.join("schema.json")).ok().and_then(|s| Schema::from_json(&s)).unwrap_or_default()));
        let last = std::fs::read_to_string(state_dir.join("last")).ok().map(|s| qdir.join(s.trim())).filter(|p| p.is_file());
        let path = last.unwrap_or_else(|| qdir.join("scratch.sql"));
        if !path.exists() {
            let _ = std::fs::write(&path, "");
        }
        let pane = Self::make_pane(&path, &schema, w, h);
        let n = schema.read().unwrap().tables.len();
        let mut app = App {
            screen: Screen::new(w, h),
            backend,
            schema,
            schema_state: Ok(if n > 0 { Some(n) } else { None }),
            pane,
            path,
            qdir,
            state_dir,
            focus: Focus::Editor,
            win: false,
            show_schema: true,
            open: HashSet::new(),
            tree_sel: 0,
            tree_top: 0,
            body: Body::Empty,
            info: String::new(),
            record: None,
            running: None,
            next_id: 0,
            note: None,
            help: false,
            ask: None,
            picker: None,
            tx,
            waker,
            quit: false,
            suspend: false,
        };
        app.refresh_schema();
        app
    }

    fn make_pane(path: &std::path::Path, schema: &Arc<RwLock<Schema>>, w: usize, h: usize) -> Pane {
        let mut pane = Pane::open(w, h, path, 1);
        let s = schema.clone();
        pane.ed.provider = Some(Box::new(move |lines, cur, manual| sql::complete(&s.read().unwrap(), lines, cur, manual)));
        pane.ed.host_names = HOST_CMDS.iter().map(|s| s.to_string()).collect();
        pane
    }

    // ---------- layout ----------

    /// (schema width, main x, main width, editor height)
    fn layout(&self) -> (usize, usize, usize, usize) {
        let (w, h) = (self.screen.w, self.screen.h);
        let sw = if self.show_schema && w >= 70 { (w / 4).clamp(24, 38) } else { 0 };
        let mx = if sw > 0 { sw + 1 } else { 0 };
        let eh = ((h - 1) * 2 / 5).max(6).min(h.saturating_sub(6));
        (sw, mx, w - mx, eh)
    }

    pub fn tick(&mut self) -> i32 {
        if self.running.is_some() {
            200
        } else {
            1000
        }
    }

    // ---------- background work ----------

    pub fn refresh_schema(&mut self) {
        if self.schema.read().unwrap().tables.is_empty() {
            self.schema_state = Ok(None);
        }
        let (b, tx, waker) = (self.backend.clone(), self.tx.clone(), self.waker);
        std::thread::spawn(move || {
            let _ = tx.send(Event::Schema(b.schema()));
            waker.wake();
        });
    }

    fn run_sql(&mut self, sql: String) {
        let ss = sql::statements(&sql);
        let risky = ss.iter().any(|s| {
            let toks: Vec<String> = sql::lex(s).iter().filter_map(|t| if let sql::T::Word(w) = &t.t { Some(w.to_ascii_uppercase()) } else { None }).collect();
            match toks.first().map(String::as_str) {
                Some("DROP") | Some("TRUNCATE") => true,
                Some("DELETE") | Some("UPDATE") => !toks.iter().any(|w| w == "WHERE"),
                _ => false,
            }
        });
        if risky {
            self.ask = Some(sql);
        } else {
            self.start(sql);
        }
    }

    fn start(&mut self, sql: String) {
        self.next_id += 1;
        let id = self.next_id;
        self.running = Some((id, Instant::now()));
        self.note = None;
        let (b, tx, waker) = (self.backend.clone(), self.tx.clone(), self.waker);
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = b.run(&sql);
            let _ = tx.send(Event::Query(id, r, t.elapsed()));
            waker.wake();
        });
    }

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Schema(Ok(s)) => {
                let n = s.tables.len();
                let _ = std::fs::write(self.state_dir.join("schema.json"), s.to_json());
                *self.schema.write().unwrap() = s;
                self.schema_state = Ok(Some(n));
            }
            Event::Schema(Err(e)) => self.schema_state = Err(e),
            Event::Query(id, r, took) => {
                if self.running.map(|x| x.0) != Some(id) {
                    return;
                }
                self.running = None;
                self.record = None;
                match r {
                    Err(e) => {
                        self.body = Body::Error(e);
                        self.info = format!("failed after {} ms", took.as_millis());
                    }
                    Ok(out) => {
                        let n = out.sets.len();
                        let pick = out.sets.iter().rposition(|s| !s.columns.is_empty()).or(n.checked_sub(1));
                        let prefix = if n > 1 { format!("{} statements · ", n) } else { String::new() };
                        match pick.map(|i| out.sets[i].clone()) {
                            Some(set) if !set.columns.is_empty() => {
                                let rows = set.rows.len();
                                self.info = format!("{}{} row{} · {}", prefix, rows, if rows == 1 { "" } else { "s" }, out.info);
                                self.body = Body::Table(Box::new(Grid::new(set)));
                            }
                            _ => {
                                self.info = format!("{}{}", prefix, out.info);
                                self.body = Body::Message("OK".into());
                            }
                        }
                        // DDL changes what completion should know
                        if self.last_sql_is_ddl() {
                            self.refresh_schema();
                        }
                    }
                }
            }
        }
    }

    fn last_sql_is_ddl(&self) -> bool {
        let src = self.pane.ed.bb().lines.join("\n");
        sql::statements(&src).iter().any(|s| {
            let u = s.to_ascii_uppercase();
            u.starts_with("CREATE") || u.starts_with("DROP") || u.starts_with("ALTER")
        })
    }

    // ---------- the editor's text ----------

    fn buffer_text(&self) -> String {
        self.pane.ed.bb().lines.join("\n")
    }

    /// What `run` should send: the visual selection, or the statement at the cursor.
    fn current_sql(&self) -> Option<String> {
        let ed = &self.pane.ed;
        let lines = &ed.bb().lines;
        let (cy, cx) = ed.cursor();
        match ed.mode {
            mvi::editor::Mode::Visual | mvi::editor::Mode::VisualLine => {
                let (a, b) = if ed.vstart <= (cy, cx) { (ed.vstart, (cy, cx)) } else { ((cy, cx), ed.vstart) };
                let text = if ed.mode == mvi::editor::Mode::VisualLine {
                    lines[a.0..=b.0].join("\n")
                } else {
                    let mut t = String::new();
                    for y in a.0..=b.0 {
                        let l = &lines[y];
                        let from = if y == a.0 { a.1.min(l.len()) } else { 0 };
                        let mut to = if y == b.0 { b.1.min(l.len()) } else { l.len() };
                        if y == b.0 {
                            to = l[to..].chars().next().map_or(to, |c| to + c.len_utf8());
                        }
                        t.push_str(&l[from..to]);
                        if y != b.0 {
                            t.push('\n');
                        }
                    }
                    t
                };
                Some(text).filter(|t| !t.trim().is_empty())
            }
            _ => {
                let off = lines.iter().take(cy).map(|l| l.len() + 1).sum::<usize>() + cx;
                sql::statement_at(&lines.join("\n"), off).map(|x| x.1)
            }
        }
    }

    fn save_file(&mut self) {
        self.pane.ed.ex("w");
        self.pane.ed.msg.clear();
    }

    fn run_current(&mut self, all: bool) {
        self.save_file();
        let sql = if all { Some(self.buffer_text()).filter(|t| !sql::statements(t).is_empty()) } else { self.current_sql() };
        match sql {
            Some(s) => self.run_sql(s),
            None => self.note = Some(("nothing to run here".into(), true)),
        }
    }

    // ---------- saved queries ----------

    fn queries(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.qdir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".sql")).map(String::from)).collect();
        v.sort_by(|a, b| (a != "scratch").cmp(&(b != "scratch")).then(a.cmp(b)));
        v
    }

    fn file_for(&self, name: &str) -> PathBuf {
        let n: String = name.trim().trim_end_matches(".sql").chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
        self.qdir.join(format!("{}.sql", n))
    }

    fn open_path(&mut self, path: PathBuf) {
        self.save_file();
        let (w, h) = (self.screen.w, self.screen.h);
        let (_, _, mw, eh) = self.layout();
        let _ = (w, h);
        self.pane = Self::make_pane(&path, &self.schema, mw, eh);
        if let Some(n) = path.file_name() {
            let _ = std::fs::write(self.state_dir.join("last"), n.to_string_lossy().as_bytes());
        }
        self.path = path;
        self.focus = Focus::Editor;
    }

    fn say(&mut self, s: impl Into<String>, err: bool) {
        self.note = Some((s.into(), err));
    }

    fn host_cmd(&mut self, cmd: &str) {
        let (name, arg) = cmd.split_once(' ').map_or((cmd, ""), |(n, a)| (n, a.trim()));
        match name {
            "run" => self.run_current(false),
            "runall" => self.run_current(true),
            "msql" => self.help = true,
            "schema" => self.show_schema = !self.show_schema,
            "refresh" => {
                self.refresh_schema();
                self.say("refreshing the schema…", false);
            }
            "save" => {
                let target = if arg.is_empty() { self.path.clone() } else { self.file_for(arg) };
                if target == self.path {
                    self.save_file();
                } else if target.exists() {
                    self.say(format!("{} already exists", target.file_stem().unwrap_or_default().to_string_lossy()), true);
                    return;
                } else {
                    let text = self.buffer_text() + "\n";
                    if let Err(e) = std::fs::write(&target, text) {
                        self.say(e.to_string(), true);
                        return;
                    }
                    self.open_path(target.clone());
                }
                self.say(format!("saved as {}", target.file_stem().unwrap_or_default().to_string_lossy()), false);
            }
            "open" if arg.is_empty() => {
                self.picker = Some(Picker::new("queries", self.queries()));
            }
            "open" => {
                let p = self.file_for(arg);
                if p.is_file() {
                    self.open_path(p);
                } else {
                    self.say(format!("no saved query {}", arg), true);
                }
            }
            "new" => {
                let name = if arg.is_empty() { (1..).map(|i| format!("query-{}", i)).find(|n| !self.file_for(n).exists()).unwrap() } else { arg.to_string() };
                let p = self.file_for(&name);
                if !p.exists() {
                    let _ = std::fs::write(&p, "");
                }
                self.open_path(p);
            }
            "rm" => {
                let p = self.file_for(arg);
                if arg.is_empty() || p == self.path {
                    self.say("name a saved query other than the one open", true);
                } else if std::fs::remove_file(&p).is_ok() {
                    self.say(format!("removed {}", arg), false);
                } else {
                    self.say(format!("no saved query {}", arg), true);
                }
            }
            "export" => self.export(arg),
            _ => {}
        }
    }

    fn export(&mut self, path: &str) {
        let Body::Table(g) = &self.body else { return self.say("no result to export", true) };
        if path.is_empty() {
            return self.say("export to which file? (:export out.csv, .tsv or .json)", true);
        }
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let mut out = String::new();
        match ext.as_str() {
            "json" => {
                out.push('[');
                for (i, r) in g.set.rows.iter().enumerate() {
                    out.push_str(if i > 0 { ",\n " } else { "\n " });
                    out.push('{');
                    for (j, v) in r.iter().enumerate() {
                        if j > 0 {
                            out.push_str(", ");
                        }
                        let val = match v.kind {
                            Kind::Null => "null".to_string(),
                            Kind::Num if v.text.parse::<f64>().is_ok() => v.text.clone(),
                            _ => mtui::json::quote(&v.text),
                        };
                        out.push_str(&format!("{}: {}", mtui::json::quote(g.set.columns.get(j).map_or("", |c| c.as_str())), val));
                    }
                    out.push('}');
                }
                out.push_str("\n]\n");
            }
            "tsv" => {
                out.push_str(&g.set.columns.join("\t"));
                out.push('\n');
                for i in 0..g.set.rows.len() {
                    out.push_str(&g.row_tsv(i));
                    out.push('\n');
                }
            }
            _ => {
                out.push_str(&g.set.columns.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(","));
                out.push('\n');
                for r in &g.set.rows {
                    out.push_str(&r.iter().map(|v| if v.kind == Kind::Null { String::new() } else { csv_field(&v.text) }).collect::<Vec<_>>().join(","));
                    out.push('\n');
                }
            }
        }
        let n = g.set.rows.len();
        match std::fs::write(path, out) {
            Ok(()) => self.say(format!("wrote {} rows to {}", n, path), false),
            Err(e) => self.say(format!("{}: {}", path, e), true),
        }
    }

    // ---------- keys ----------

    pub fn handle_key(&mut self, k: Key) {
        if self.help {
            if !matches!(k, Key::Mouse(_)) {
                self.help = false;
            }
            return;
        }
        if let Some(sql) = self.ask.take() {
            match k {
                Key::Char('y') | Key::Char('Y') => self.start(sql),
                Key::Mouse(_) => self.ask = Some(sql),
                _ => {}
            }
            return;
        }
        if let Some(p) = &mut self.picker {
            match p.key(&k) {
                Pick::Continue => {}
                Pick::Cancel => self.picker = None,
                Pick::Accept => {
                    let pick = p.selected().cloned();
                    self.picker = None;
                    if let Some(n) = pick {
                        let path = self.file_for(&n);
                        self.open_path(path);
                    }
                }
            }
            return;
        }
        match k {
            Key::Alt('\n') | Key::Ctrl('g') => return self.run_current(false),
            Key::Alt('h') => return self.go(Focus::Schema),
            Key::Alt('j') => return self.go(Focus::Results),
            Key::Alt('k') | Key::Alt('l') => return self.go(Focus::Editor),
            Key::Mouse(m) => return self.mouse(m),
            _ => {}
        }
        match self.focus {
            Focus::Editor => self.editor_key(k),
            Focus::Results => self.results_key(k),
            Focus::Schema => self.schema_key(k),
        }
    }

    fn go(&mut self, f: Focus) {
        if f == Focus::Schema {
            self.show_schema = true;
        }
        self.focus = f;
        self.win = false;
    }

    fn editor_key(&mut self, k: Key) {
        if self.win {
            self.win = false;
            match k {
                Key::Char('h') | Key::Ctrl('h') => self.go(Focus::Schema),
                Key::Char('j') | Key::Ctrl('j') => self.go(Focus::Results),
                _ => {}
            }
            return;
        }
        if k == Key::Ctrl('w') && self.pane.ed.mode == mvi::editor::Mode::Normal {
            self.win = true;
            return;
        }
        self.pane.key(k);
        for c in std::mem::take(&mut self.pane.ed.host_cmds) {
            self.host_cmd(&c);
        }
        if self.pane.finished() {
            self.quit = true;
        }
    }

    fn feed_editor(&mut self, text: &str) {
        let normal = self.pane.ed.mode == mvi::editor::Mode::Normal;
        if normal {
            self.pane.key(Key::Char('i'));
        }
        self.pane.key(Key::Paste(text.to_string()));
        if normal {
            self.pane.key(Key::Esc);
        }
        self.focus = Focus::Editor;
    }

    fn results_key(&mut self, k: Key) {
        let visible = self.screen.h.saturating_sub(self.layout().3 + 4).max(1);
        if let Some(rec) = &mut self.record {
            let Body::Table(g) = &mut self.body else { return };
            match k {
                Key::Esc | Key::Char('q') | Key::Enter => self.record = None,
                Key::Char('j') | Key::Down => rec.scroll += 1,
                Key::Char('k') | Key::Up => rec.scroll = rec.scroll.saturating_sub(1),
                Key::Char('n') | Key::Right | Key::Char('l') if g.row + 1 < g.set.rows.len() => {
                    g.row += 1;
                    rec.scroll = 0;
                }
                Key::Char('p') | Key::Left | Key::Char('h') if g.row > 0 => {
                    g.row -= 1;
                    rec.scroll = 0;
                }
                Key::Char('y') => {
                    let text = g.set.columns.iter().zip(&g.set.rows[g.row]).map(|(c, v)| format!("{}: {}", c, v.text)).collect::<Vec<_>>().join("\n");
                    self.screen.osc52(&text);
                    self.note = Some(("copied the row".into(), false));
                }
                _ => {}
            }
            return;
        }
        let Body::Table(g) = &mut self.body else {
            if matches!(k, Key::Esc | Key::Char('q') | Key::Char('i')) {
                self.focus = Focus::Editor;
            }
            if k == Key::Char(':') {
                self.focus = Focus::Editor;
                self.pane.key(k);
            }
            return;
        };
        let (nr, nc) = (g.set.rows.len(), g.set.columns.len());
        match k {
            Key::Char('j') | Key::Down => g.row = (g.row + 1).min(nr.saturating_sub(1)),
            Key::Char('k') | Key::Up => g.row = g.row.saturating_sub(1),
            Key::Char('h') | Key::Left => g.col = g.col.saturating_sub(1),
            Key::Char('l') | Key::Right => g.col = (g.col + 1).min(nc.saturating_sub(1)),
            Key::Char('g') | Key::Home => g.row = 0,
            Key::Char('G') | Key::End => g.row = nr.saturating_sub(1),
            Key::Char('0') => g.col = 0,
            Key::Char('$') => g.col = nc.saturating_sub(1),
            Key::Ctrl('d') | Key::PageDown => g.row = (g.row + visible / 2).min(nr.saturating_sub(1)),
            Key::Ctrl('u') | Key::PageUp => g.row = g.row.saturating_sub(visible / 2),
            Key::Enter if nr > 0 => self.record = Some(Record { scroll: 0 }),
            Key::Char('y') => {
                let t = g.cell().to_string();
                self.screen.osc52(&t);
                self.note = Some(("copied the cell".into(), false));
            }
            Key::Char('Y') => {
                let t = g.row_tsv(g.row);
                self.screen.osc52(&t);
                self.note = Some(("copied the row".into(), false));
            }
            Key::Esc | Key::Char('q') | Key::Char('i') => self.focus = Focus::Editor,
            Key::Char(':') => {
                self.focus = Focus::Editor;
                self.pane.key(k);
            }
            _ => {}
        }
    }

    /// The tree's rows: (table index, column index or None).
    fn tree_rows(&self) -> Vec<(usize, Option<usize>)> {
        let s = self.schema.read().unwrap();
        let mut v = Vec::new();
        for (i, t) in s.tables.iter().enumerate() {
            v.push((i, None));
            if self.open.contains(&t.full_name()) {
                v.extend((0..t.columns.len()).map(|c| (i, Some(c))));
            }
        }
        v
    }

    fn schema_key(&mut self, k: Key) {
        let rows = self.tree_rows();
        let n = rows.len();
        let cur = rows.get(self.tree_sel).copied();
        let name_of = |app: &App, (t, c): (usize, Option<usize>)| {
            let s = app.schema.read().unwrap();
            let t = &s.tables[t];
            c.map_or_else(|| sql::qualified(t), |c| sql::ident(&t.columns[c].name))
        };
        match k {
            Key::Char('j') | Key::Down => self.tree_sel = (self.tree_sel + 1).min(n.saturating_sub(1)),
            Key::Char('k') | Key::Up => self.tree_sel = self.tree_sel.saturating_sub(1),
            Key::Char('g') | Key::Home => self.tree_sel = 0,
            Key::Char('G') | Key::End => self.tree_sel = n.saturating_sub(1),
            Key::Ctrl('d') | Key::PageDown => self.tree_sel = (self.tree_sel + 10).min(n.saturating_sub(1)),
            Key::Ctrl('u') | Key::PageUp => self.tree_sel = self.tree_sel.saturating_sub(10),
            Key::Enter | Key::Char(' ') | Key::Char('l') | Key::Right => match cur {
                Some((t, None)) => {
                    let name = self.schema.read().unwrap().tables[t].full_name();
                    if !self.open.remove(&name) {
                        self.open.insert(name);
                    }
                }
                Some(c) if k == Key::Enter => {
                    let t = name_of(self, c);
                    self.feed_editor(&t);
                }
                _ => {}
            },
            Key::Char('h') | Key::Left => {
                if let Some((t, _)) = cur {
                    let name = self.schema.read().unwrap().tables[t].full_name();
                    self.open.remove(&name);
                    self.tree_sel = self.tree_rows().iter().position(|r| *r == (t, None)).unwrap_or(0);
                }
            }
            Key::Char('i') | Key::Char('a') => {
                if let Some(c) = cur {
                    let t = name_of(self, c);
                    self.feed_editor(&t);
                }
            }
            Key::Char('p') => {
                if let Some((t, _)) = cur {
                    let name = sql::qualified(&self.schema.read().unwrap().tables[t]);
                    self.run_sql(format!("SELECT * FROM {} LIMIT 100;", name));
                }
            }
            Key::Char('r') => {
                self.refresh_schema();
                self.say("refreshing the schema…", false);
            }
            Key::Esc | Key::Char('q') => self.focus = Focus::Editor,
            Key::Char(':') => {
                self.focus = Focus::Editor;
                self.pane.key(k);
            }
            _ => {}
        }
        self.tree_sel = self.tree_sel.min(self.tree_rows().len().saturating_sub(1));
    }

    fn mouse(&mut self, m: mtui::term::Mouse) {
        let (sw, mx, _mw, eh) = self.layout();
        let in_editor = m.x >= mx && m.y >= 1 && m.y < 1 + eh;
        let in_results = m.x >= mx && m.y >= 1 + eh;
        let in_schema = sw > 0 && m.x < sw && m.y >= 1;
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let d: isize = if m.kind == MouseKind::WheelUp { -3 } else { 3 };
                if in_results {
                    if let Body::Table(g) = &mut self.body {
                        g.row = (g.row as isize + d).clamp(0, g.set.rows.len().saturating_sub(1) as isize) as usize;
                    }
                } else if in_schema {
                    self.tree_sel = (self.tree_sel as isize + d).clamp(0, self.tree_rows().len().saturating_sub(1) as isize) as usize;
                } else if in_editor {
                    let mut m = m;
                    m.x -= mx;
                    m.y -= 1;
                    self.pane.key(Key::Mouse(m));
                }
            }
            MouseKind::Press(0) => {
                if in_editor {
                    self.focus = Focus::Editor;
                    let mut m = m;
                    m.x -= mx;
                    m.y -= 1;
                    self.pane.key(Key::Mouse(m));
                } else if in_schema {
                    self.focus = Focus::Schema;
                    let i = self.tree_top + m.y - 2;
                    if m.y >= 2 && i < self.tree_rows().len() {
                        self.tree_sel = i;
                    }
                } else if in_results {
                    self.focus = Focus::Results;
                    if let Body::Table(g) = &mut self.body {
                        let y0 = 1 + eh;
                        if m.y >= y0 + 2 {
                            let r = g.top + m.y - y0 - 2;
                            if r < g.set.rows.len() {
                                g.row = r;
                            }
                            let gw = g.set.rows.len().to_string().len() + 1;
                            let mut x = mx + gw + 1;
                            for c in g.left..g.widths.len() {
                                if m.x < x + g.widths[c] + 3 {
                                    g.col = c;
                                    break;
                                }
                                x += g.widths[c] + 3;
                            }
                        }
                    }
                }
            }
            _ => {
                if in_editor && self.focus == Focus::Editor {
                    let mut m = m;
                    m.x = m.x.saturating_sub(mx);
                    m.y = m.y.saturating_sub(1);
                    self.pane.key(Key::Mouse(m));
                }
            }
        }
    }

    // ---------- drawing ----------

    pub fn render(&mut self) {
        use mtui::screen::{Style, BOLD};
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        if w < 40 || h < 12 {
            self.screen.puts(0, 0, "too small", Style::default(), w);
            self.screen.flush(None);
            return;
        }
        let (sw, mx, mw, eh) = self.layout();
        self.title_bar(w);
        if sw > 0 {
            self.draw_schema(sw, h);
        }
        let cur = self.pane.draw(&mut self.screen, (mx, 1), (mw, eh));
        self.draw_results(mx, 1 + eh, mw, h - 1 - eh);
        let mut cursor = if self.focus == Focus::Editor && !self.help { cur } else { None };
        if let Some(p) = &mut self.picker {
            let (x, y) = p.draw(&mut self.screen, h);
            cursor = Some((x, y, true));
        }
        if let Some(sql) = &self.ask {
            let first = sql.trim().lines().next().unwrap_or("");
            let msg = format!(" This changes data without a WHERE (or drops something): {}  Run it? y/N ", clip(first, w.saturating_sub(60)));
            self.screen.fill(0, w, h - 1, Style::new(16, C_BAD, BOLD));
            self.screen.puts(0, h - 1, &msg, Style::new(16, C_BAD, BOLD), w);
            cursor = None;
        }
        if self.help {
            let lines: Vec<&str> = HELP.lines().collect();
            let y0 = (h.saturating_sub(lines.len() + 2)) / 2;
            let ww = lines.iter().map(|l| disp_w(l)).max().unwrap_or(0).min(w - 2) + 2;
            let x0 = (w - ww) / 2;
            for y in y0..(y0 + lines.len() + 2).min(h) {
                self.screen.fill(x0, x0 + ww, y, Style::new(252, 234, 0));
            }
            for (i, l) in lines.iter().enumerate() {
                self.screen.puts(x0 + 1, y0 + 1 + i, l, Style::new(if i == 0 { ACCENT } else { 252 }, 234, if i == 0 { BOLD } else { 0 }), x0 + ww);
            }
            cursor = None;
        }
        self.screen.flush(cursor);
    }

    fn title_bar(&mut self, w: usize) {
        use mtui::screen::{Style, BOLD};
        self.screen.fill(0, w, 0, Style::new(250, BG_BAR, 0));
        let x = self.screen.puts(0, 0, " msql ", Style::new(16, ACCENT, BOLD), w);
        let x = self.screen.puts(x + 1, 0, &self.backend.title(), Style::new(255, BG_BAR, BOLD), w);
        let file = self.path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let modified = if self.pane.ed.bb().dirty { " +" } else { "" };
        let x = self.screen.puts(x + 2, 0, &format!("{}{}", file, modified), Style::new(C_HEAD, BG_BAR, 0), w);
        let (txt, c) = match &self.schema_state {
            Ok(Some(n)) => (format!("{} tables", n), FG_DIM),
            Ok(None) => ("loading schema…".to_string(), FG_DIM),
            Err(e) => (format!("schema: {}", clip(e, 60)), C_BAD),
        };
        self.screen.puts(x + 2, 0, &txt, Style::new(c, BG_BAR, 0), w);
        let hint = "Alt-Enter run · :msql help ";
        if w > 90 {
            self.screen.puts(w - disp_w(hint), 0, hint, Style::new(FG_DIM, BG_BAR, 0), w);
        }
    }

    fn draw_schema(&mut self, sw: usize, h: usize) {
        use mtui::screen::{Style, BOLD};
        let focused = self.focus == Focus::Schema;
        for y in 1..h {
            self.screen.put(sw, y, '│', Style::new(238, 0, 0));
        }
        self.screen.fill(0, sw, 1, Style::new(250, BG_BAR, 0));
        self.screen.puts(1, 1, "Schema", Style::new(if focused { ACCENT } else { 250 }, BG_BAR, BOLD), sw);
        let rows = self.tree_rows();
        let vis = h.saturating_sub(2);
        if self.tree_sel < self.tree_top {
            self.tree_top = self.tree_sel;
        }
        if self.tree_sel >= self.tree_top + vis {
            self.tree_top = self.tree_sel + 1 - vis;
        }
        let s = self.schema.read().unwrap();
        if s.tables.is_empty() {
            let msg = match &self.schema_state {
                Err(_) => "(schema failed to load)",
                Ok(None) => "(loading…)",
                Ok(Some(_)) => "(no tables)",
            };
            self.screen.puts(1, 2, msg, Style::new(FG_DIM, 0, 0), sw);
        }
        for (k, &(ti, ci)) in rows.iter().enumerate().skip(self.tree_top).take(vis) {
            let y = 2 + k - self.tree_top;
            let sel = k == self.tree_sel;
            let bg = if sel && focused { BG_SEL } else if sel { BG_ROW } else { 0 };
            self.screen.fill(0, sw, y, Style::new(0, bg, 0));
            let t = &s.tables[ti];
            match ci {
                None => {
                    let open = self.open.contains(&t.full_name());
                    let x = self.screen.puts(1, y, if open { "▾ " } else { "▸ " }, Style::new(FG_DIM, bg, 0), sw);
                    let x = self.screen.puts(x, y, &clip(&t.full_name(), sw.saturating_sub(x + 1)), Style::new(C_OK, bg, if t.view { mtui::screen::ITALIC } else { BOLD }), sw);
                    let n = format!(" {}", t.columns.len());
                    self.screen.puts(x, y, &n, Style::new(FG_DIM, bg, 0), sw);
                }
                Some(c) => {
                    let col = &t.columns[c];
                    let x = self.screen.puts(4, y, &clip(&col.name, sw.saturating_sub(5)), Style::new(if col.pk { ACCENT } else { 252 }, bg, 0), sw);
                    let ty = format!(" {}{}", col.ty.to_lowercase(), if col.pk { " pk" } else { "" });
                    self.screen.puts(x, y, &clip(&ty, sw.saturating_sub(x + 1)), Style::new(FG_DIM, bg, 0), sw);
                }
            }
        }
    }

    fn draw_results(&mut self, x0: usize, y0: usize, w: usize, h: usize) {
        use mtui::screen::{Style, BOLD, ITALIC};
        let focused = self.focus == Focus::Results;
        self.screen.fill(x0, x0 + w, y0, Style::new(250, BG_BAR, 0));
        let x = self.screen.puts(x0, y0, " Results ", Style::new(16, if focused { ACCENT } else { 244 }, BOLD), x0 + w);
        let status = match (&self.running, &self.note) {
            (Some((_, t)), _) => {
                let sp = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(t.elapsed().as_millis() / 100) as usize % 10];
                (format!("{} running… {:.1}s", sp, t.elapsed().as_secs_f32()), ACCENT)
            }
            (None, Some((m, err))) => (m.clone(), if *err { C_BAD } else { C_OK }),
            _ => (self.info.clone(), FG_DIM),
        };
        self.screen.puts(x + 1, y0, &clip(&status.0, w.saturating_sub(x - x0 + 2)), Style::new(status.1, BG_BAR, 0), x0 + w);
        let body_h = h.saturating_sub(1);
        let by = y0 + 1;
        match &mut self.body {
            Body::Empty => {
                self.screen.puts(x0 + 2, by + 1, "Alt-Enter runs the statement under the cursor.", Style::new(FG_DIM, 0, ITALIC), x0 + w);
                self.screen.puts(x0 + 2, by + 2, "Press Alt-h for the schema, Ctrl-Space for completion, :msql for all keys.", Style::new(FG_DIM, 0, ITALIC), x0 + w);
            }
            Body::Message(m) => {
                self.screen.puts(x0 + 2, by + 1, m, Style::new(C_OK, 0, BOLD), x0 + w);
            }
            Body::Error(e) => {
                for (i, l) in wrap(e, w.saturating_sub(4)).iter().take(body_h).enumerate() {
                    self.screen.puts(x0 + 2, by + i, l, Style::new(C_BAD, 0, 0), x0 + w);
                }
            }
            Body::Table(g) => {
                if let Some(rec) = &mut self.record {
                    // one row, a column per line, long values wrapped
                    let kw = g.set.columns.iter().map(|c| disp_w(c)).max().unwrap_or(0).min(w / 3);
                    let mut lines: Vec<(Option<usize>, String)> = Vec::new();
                    for (i, v) in g.set.rows[g.row].iter().enumerate() {
                        for (j, l) in wrap(&v.text, w.saturating_sub(kw + 5)).into_iter().enumerate() {
                            lines.push((if j == 0 { Some(i) } else { None }, l));
                        }
                    }
                    rec.scroll = rec.scroll.min(lines.len().saturating_sub(1));
                    self.screen.puts(x0 + 2, by, &format!("row {} of {}", g.row + 1, g.set.rows.len()), Style::new(FG_DIM, 0, ITALIC), x0 + w);
                    for (k, (ci, l)) in lines.iter().skip(rec.scroll).take(body_h.saturating_sub(1)).enumerate() {
                        let y = by + 1 + k;
                        if let Some(c) = ci {
                            self.screen.puts(x0 + 2, y, &clip(&g.set.columns[*c], kw), Style::new(C_HEAD, 0, BOLD), x0 + w);
                        }
                        let null = ci.is_some_and(|c| g.set.rows[g.row][c].kind == Kind::Null);
                        self.screen.puts(x0 + 3 + kw, y, l, Style::new(if null { 244 } else { 252 }, 0, if null { ITALIC } else { 0 }), x0 + w);
                    }
                    return;
                }
                let nr = g.set.rows.len();
                let vis = body_h.saturating_sub(1).max(1);
                g.top = g.top.min(g.row);
                if g.row >= g.top + vis {
                    g.top = g.row + 1 - vis;
                }
                let gw = nr.to_string().len() + 1;
                // keep the selected column on screen
                let avail = w.saturating_sub(gw + 1);
                g.left = g.left.min(g.col);
                loop {
                    let used: usize = (g.left..=g.col).map(|c| g.widths[c] + 3).sum();
                    if used <= avail || g.left >= g.col {
                        break;
                    }
                    g.left += 1;
                }
                let hy = by;
                self.screen.fill(x0, x0 + w, hy, Style::new(C_HEAD, 235, BOLD));
                let mut x = x0 + gw + 1;
                for c in g.left..g.widths.len() {
                    if x >= x0 + w {
                        break;
                    }
                    let name = clip(&g.set.columns[c], g.widths[c]);
                    let st = Style::new(if c == g.col && focused { ACCENT } else { C_HEAD }, 235, BOLD | if c == g.col { mtui::screen::UNDERLINE } else { 0 });
                    self.screen.puts(x, hy, &name, st, x0 + w);
                    x += g.widths[c] + 3;
                }
                if nr == 0 {
                    self.screen.puts(x0 + 2, hy + 1, "(no rows)", Style::new(FG_DIM, 0, ITALIC), x0 + w);
                }
                for (k, r) in g.set.rows.iter().enumerate().skip(g.top).take(vis) {
                    let y = hy + 1 + k - g.top;
                    let sel = k == g.row;
                    let bg = if sel { BG_ROW } else { 0 };
                    self.screen.fill(x0, x0 + w, y, Style::new(0, bg, 0));
                    self.screen.puts(x0, y, &format!("{:>width$}", k + 1, width = gw), Style::new(240, bg, 0), x0 + w);
                    let mut x = x0 + gw + 1;
                    for c in g.left..g.widths.len() {
                        if x >= x0 + w {
                            break;
                        }
                        let v = &r[c.min(r.len().saturating_sub(1))];
                        let text = clip(&v.text, g.widths[c]);
                        let pad = if g.numeric[c] { g.widths[c].saturating_sub(disp_w(&text)) } else { 0 };
                        let here = sel && c == g.col;
                        let (fg, attr) = match v.kind {
                            Kind::Null => (244, ITALIC),
                            Kind::Num => (C_NUM, 0),
                            Kind::Text => (252, 0),
                        };
                        let cbg = if here && focused { BG_SEL } else { bg };
                        if here && focused {
                            self.screen.fill(x, (x + g.widths[c]).min(x0 + w), y, Style::new(0, cbg, 0));
                        }
                        self.screen.puts(x + pad, y, &text, Style::new(if here && focused { 255 } else { fg }, cbg, attr), x0 + w);
                        self.screen.puts(x + g.widths[c] + 1, y, "│", Style::new(238, bg, 0), x0 + w);
                        x += g.widths[c] + 3;
                    }
                }
            }
        }
    }
}
