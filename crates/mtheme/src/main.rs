mod targets;

use mtui::screen::{Screen, Style, BOLD, ITALIC};
use mtui::term::{self, Key};
use mtui::{syntax, theme};
use std::sync::atomic::Ordering;
use targets::{Kind, Target};

const FG_DIM: u8 = 246;
const BG_BAR: u8 = 236;
const BG_SEL: u8 = 237;
const ACCENT: u8 = 180;

fn usage() -> ! {
    println!(
        "usage: mtheme                         pick a theme for each app, with a preview
       mtheme list                    the themes and what each app / terminal uses
       mtheme set TARGET THEME        TARGET is an app (mvi mmail mjira mslack mgh),
                                      a terminal (kitty ghostty alacritty) or 'all'
       mtheme setup TARGET            let a terminal's config read mtheme's colour file

Apps remember their theme in ~/.config/mtui/themes and switch within a second
of it changing, even while running (Ctrl-T in an app steps to the next theme).
Terminals get a colour file (kitty: ~/.config/kitty/current-theme.conf) and are
asked to reload. MTUI_THEME=NAME overrides the saved theme for one run."
    );
    std::process::exit(0);
}

fn theme_arg(s: &str) -> usize {
    theme::find(s).unwrap_or_else(|| {
        eprintln!("mtheme: unknown theme '{}' (mtheme list)", s);
        std::process::exit(1);
    })
}

fn set_all(i: usize) -> Vec<Result<String, String>> {
    let _ = theme::save("*", i);
    targets::all().iter().map(|t| targets::apply(t, i)).collect()
}

fn cli(args: &[String]) {
    match args[0].as_str() {
        "-h" | "--help" => usage(),
        "list" => {
            println!("themes:");
            for t in theme::THEMES.iter() {
                println!("  {:<18} {}", t.id, t.name);
            }
            println!("\ncurrent:");
            for t in targets::all() {
                let cur = targets::current(&t).map(|i| theme::THEMES[i].id).unwrap_or("-");
                println!("  {:<10} {}", t.key, cur);
            }
        }
        "set" if args.len() == 3 => {
            let i = theme_arg(&args[2]);
            let res = if args[1] == "all" {
                set_all(i)
            } else {
                match targets::find(&args[1]) {
                    Some(t) => vec![targets::apply(&t, i)],
                    None => {
                        eprintln!("mtheme: unknown target '{}' (mtheme list)", args[1]);
                        std::process::exit(1);
                    }
                }
            };
            for r in res {
                match r {
                    Ok(m) => println!("{}", m),
                    Err(e) => eprintln!("mtheme: {}", e),
                }
            }
        }
        "setup" if args.len() == 2 => match targets::find(&args[1]) {
            Some(t) => match targets::install(&t) {
                Ok(m) => println!("{}", m),
                Err(e) => eprintln!("mtheme: {}", e),
            },
            None => eprintln!("mtheme: unknown target '{}'", args[1]),
        },
        _ => usage(),
    }
}

struct App {
    screen: Screen,
    rows: Vec<Target>,
    sel: usize,
    msg: String,
    err: bool,
    quit: bool,
}

impl App {
    fn theme_of(&self, i: usize) -> usize {
        targets::current(&self.rows[i]).unwrap_or(0)
    }

    fn say(&mut self, r: Result<String, String>) {
        match r {
            Ok(m) => (self.msg, self.err) = (m, false),
            Err(e) => (self.msg, self.err) = (e, true),
        }
    }

    /// Step the highlighted row's theme.
    fn step(&mut self, d: isize) {
        let n = theme::THEMES.len() as isize;
        let cur = targets::current(&self.rows[self.sel]).map(|i| i as isize).unwrap_or(-1);
        let next = if cur < 0 { 0 } else { (cur + d).rem_euclid(n) } as usize;
        let r = targets::apply(&self.rows[self.sel], next);
        self.say(r);
        if let Some(h) = targets::needs_setup(&self.rows[self.sel]) {
            self.msg = format!("{}  —  {}", self.msg, h);
        }
    }

    fn all_like_row(&mut self) {
        let i = self.theme_of(self.sel);
        let res = set_all(i);
        let bad: Vec<String> = res.into_iter().filter_map(|r| r.err()).collect();
        self.say(if bad.is_empty() { Ok(format!("all: {}", theme::THEMES[i].name)) } else { Err(bad.join("; ")) });
    }

    fn key(&mut self, k: Key) {
        let n = self.rows.len();
        match k {
            Key::Char('q') | Key::Esc | Key::Ctrl('c') => self.quit = true,
            Key::Char('j') | Key::Down => self.sel = (self.sel + 1) % n,
            Key::Char('k') | Key::Up => self.sel = (self.sel + n - 1) % n,
            Key::Char('g') | Key::Home => self.sel = 0,
            Key::Char('G') | Key::End => self.sel = n - 1,
            Key::Char('l') | Key::Right | Key::Enter | Key::Char(' ') | Key::Tab => self.step(1),
            Key::Char('h') | Key::Left | Key::BackTab => self.step(-1),
            Key::Char('a') => self.all_like_row(),
            Key::Char('I') => {
                let r = targets::install(&self.rows[self.sel]);
                self.say(r);
            }
            Key::Char(c) if c.is_ascii_digit() && c != '0' => {
                let i = c as usize - '1' as usize;
                if i < theme::THEMES.len() {
                    let r = targets::apply(&self.rows[self.sel], i);
                    self.say(r);
                }
            }
            _ => {}
        }
    }

    fn render(&mut self) {
        // the whole screen previews the highlighted row's theme
        let shown = self.theme_of(self.sel);
        theme::set(shown);
        let (w, h) = (self.screen.w, self.screen.h);
        self.screen.clear();
        self.screen.fill(0, w, 0, Style::new(252, BG_BAR, 0));
        let x = self.screen.puts(0, 0, " mtheme ", Style::new(16, ACCENT, BOLD), w) + 1;
        self.screen.puts(x, 0, "h l change · 1-5 pick · a all like this · I set up terminal · q quit", Style::new(250, BG_BAR, 0), w);

        let wide = w >= 84;
        let list_w = if wide { 44 } else { w };
        self.screen.puts(1, 2, "theme per app", Style::new(ACCENT, 0, BOLD), list_w);
        for (i, t) in self.rows.iter().enumerate() {
            let y = 3 + i;
            if y + 2 >= h {
                break;
            }
            let sel = i == self.sel;
            let bg = if sel { BG_SEL } else { 0 };
            self.screen.fill(0, list_w, y, Style::new(0, bg, 0));
            let kind = if t.kind == Kind::App { "app" } else { "terminal" };
            self.screen.puts(1, y, if sel { "›" } else { " " }, Style::new(ACCENT, bg, BOLD), list_w);
            self.screen.puts(3, y, t.key, Style::new(255, bg, if sel { BOLD } else { 0 }), list_w);
            self.screen.puts(14, y, kind, Style::new(FG_DIM, bg, ITALIC), list_w);
            let name = match targets::current(t) {
                Some(c) => theme::THEMES[c].name,
                None => "—",
            };
            let warn = targets::needs_setup(t).is_some();
            self.screen.puts(24, y, name, Style::new(if targets::current(t).is_some() { 252 } else { FG_DIM }, bg, 0), list_w);
            if warn {
                self.screen.puts(list_w.saturating_sub(2), y, "!", Style::new(179, bg, BOLD), list_w);
            }
        }
        let ty = 4 + self.rows.len();
        if ty + 8 < h {
            self.screen.puts(1, ty, "themes", Style::new(ACCENT, 0, BOLD), list_w);
            for (i, t) in theme::THEMES.iter().enumerate() {
                let on = i == shown;
                self.screen.puts(1, ty + 1 + i, &format!("{} {} {}", if on { "●" } else { "○" }, i + 1, t.name), Style::new(if on { 255 } else { 250 }, 0, if on { BOLD } else { 0 }), list_w);
            }
        }
        if wide {
            self.preview(46, 2, w - 47, h.saturating_sub(4));
        }
        let y = h - 1;
        self.screen.fill(0, w, y, Style::new(252, BG_BAR, 0));
        let tag = if self.err { " ERROR " } else { " THEME " };
        let x = self.screen.puts(0, y, tag, Style::new(16, if self.err { 203 } else { 110 }, BOLD), w) + 1;
        self.screen.puts(x, y, &self.msg, Style::new(if self.err { 203 } else { 252 }, BG_BAR, 0), w);
        self.screen.flush(None);
    }

    /// A small mock of the apps, drawn the way they draw.
    fn preview(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let s = &mut self.screen;
        let end = x + w;
        let name = theme::current_def().name;
        s.puts(x, y, &format!("preview — {}", name), Style::new(ACCENT, 0, BOLD), end);
        let mut row = y + 2;
        let mut line = |s: &mut Screen, parts: &[(&str, Style)], bg: u8| {
            if row >= y + h {
                return;
            }
            s.fill(x, end, row, Style::new(0, bg, 0));
            let mut cx = x + 1;
            for (t, st) in parts {
                let st = if st.bg == 0 { Style::new(st.fg, bg, st.attr) } else { *st };
                cx = s.puts(cx, row, t, st, end);
            }
            row += 1;
        };
        let k = syntax::style(syntax::KEYWORD);
        let t = syntax::style(syntax::TYPE);
        let f = syntax::style(syntax::FUNC);
        let st = syntax::style(syntax::STRING);
        let n = syntax::style(syntax::NUMBER);
        let c = syntax::style(syntax::COMMENT);
        let p = Style::new(252, 0, 0);
        line(s, &[(" NORMAL ", Style::new(16, 110, BOLD)), (" src/main.rs [+]", Style::new(252, BG_BAR, BOLD))], BG_BAR);
        line(s, &[("// pick your colours", c)], 0);
        line(s, &[("fn ", k), ("main", f), ("() -> ", p), ("Result", t), ("<()> {", p)], 235);
        line(s, &[("    let ", k), ("name", p), (" = ", p), ("\"evergarden\"", st), (";", p)], 0);
        line(s, &[("    let ", k), ("n", p), (": ", p), ("u32", t), (" = ", p), ("42", n), (";", p)], 0);
        line(s, &[("}", p)], 0);
        line(s, &[], 0);
        line(s, &[("+ added line", Style::new(252, 0, 0))], 22);
        line(s, &[("- removed line", Style::new(252, 0, 0))], 52);
        line(s, &[], 0);
        line(s, &[("selected row", Style::new(255, 0, BOLD)), ("  · dim hint", Style::new(FG_DIM, 0, 0))], BG_SEL);
        line(s, &[("ok ", Style::fg(114)), ("warn ", Style::fg(179)), ("error ", Style::fg(203)), ("link ", Style::fg(75)), ("mention ", Style::fg(117)), ("me", Style::fg(215))], 0);
        line(s, &[(" INSERT ", Style::new(16, 114, BOLD)), (" VISUAL ", Style::new(16, 176, BOLD)), (" COMMAND ", Style::new(16, 180, BOLD)), (" PICK ", Style::new(16, 108, BOLD))], 0);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return cli(&args);
    }
    if !term::is_tty() {
        usage();
    }
    term::install_panic_hook("mtheme");
    let (w, h) = term::size();
    let mut app = App { screen: Screen::new(w, h), rows: targets::all(), sel: 0, msg: "changes apply to running apps within a second".into(), err: false, quit: false };
    if let Err(e) = term::enable_raw() {
        eprintln!("mtheme: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new();
    while !app.quit {
        app.render();
        if let Some(k) = input.next_key(-1) {
            app.key(k);
        }
        if term::RESIZED.swap(false, Ordering::Relaxed) {
            let (w, h) = term::size();
            app.screen.resize(w, h);
        }
    }
    term::disable_raw();
}
