mod app;
mod model;

use mtui::term;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

const STARTER: &str = "\
# Grading rules and scores for one course. Edit here or in mgrade.
# `category NAME WEIGHT%` starts a category; the indented lines under it are
# its items, `NAME SCORE` where SCORE is 85, 17/20, ~90 (a what-if) or - (not
# graded yet). `n=` counts items not yet named, `drop=` drops the lowest.
course My course
scale A 93, A- 90, B+ 87, B 83, B- 80, C+ 77, C 73, C- 70, D+ 67, D 60

category Homework 40% n=5 drop=1
  HW1 -
category Exams 50% group=exams
  Exam 1 -
category Participation 10%
  Participation -
";

fn usage() -> ! {
    println!(
        "usage: mgrade [FILE]            open a course file (else the only *.grades here or in ~/.local/share/mgrade)
       mgrade -p FILE           print where you stand and what you need, then exit
       mgrade --new FILE        write a starter course file

A course file holds the grading rules and your scores in plain text; the
README shows the format. In the viewer: e or a digit enters a score, o adds an
item, w turns a score into a what-if, a sets what the rest is assumed to score,
u undoes, E edits the file in $EDITOR (mvi), ? shows every key."
    );
    std::process::exit(0);
}

fn candidates() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(".")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/mgrade"));
    }
    let mut v = vec![];
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "grades") {
                    v.push(p);
                }
            }
        }
    }
    v.sort();
    v
}

fn choose() -> PathBuf {
    let c = candidates();
    match c.len() {
        0 => {
            eprintln!("mgrade: no *.grades file here; give a FILE or make one with `mgrade --new FILE`");
            std::process::exit(1);
        }
        1 => c[0].clone(),
        _ => {
            for (i, p) in c.iter().enumerate() {
                println!("{}) {}", i + 1, p.display());
            }
            print!("which? ");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            match s.trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= c.len() => c[n - 1].clone(),
                _ => std::process::exit(1),
            }
        }
    }
}

fn load(path: &Path) -> model::Course {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("mgrade: {}: {}", path.display(), e);
        std::process::exit(1);
    });
    model::parse(&src).unwrap_or_else(|e| {
        eprintln!("mgrade: {}: {}", path.display(), e);
        std::process::exit(1);
    })
}

fn print_plain(c: &model::Course) {
    use model::*;
    println!("{}", if c.title.is_empty() { "(untitled course)" } else { &c.title });
    let cur = c.eval(Mode::Current);
    let tw: f64 = c.cats.iter().map(|k| k.weight).sum();
    println!();
    println!("{:<28} {:>6} {:>8} {:>7}", "category", "weight", "average", "points");
    for (k, calc) in c.cats.iter().zip(&cur.cats) {
        let avg = calc.avg.map_or("-".to_string(), |a| format!("{:.2}", a));
        let pts = calc.avg.map_or("-".to_string(), |a| format!("{:.2}", a * k.weight / tw));
        println!("{:<28} {:>5}% {:>8} {:>7}", k.name, num(k.weight), avg, pts);
        let empty = k.items.iter().filter(|i| i.name.is_empty() && i.score == Score::None).count();
        for (i, it) in k.items.iter().enumerate() {
            if it.name.is_empty() && it.score == Score::None {
                continue;
            }
            let name = if it.name.is_empty() { format!("#{}", i + 1) } else { it.name.clone() };
            let tag = if calc.dropped.contains(&i) { " (dropped)" } else { "" };
            println!("  {:<26} {:>6} {:>8}{}", name, it.score.show(), it.score.pct().map_or(String::new(), |p| format!("{:.1}%", p)), tag);
        }
        if empty > 0 {
            println!("  ({} more not graded yet)", empty);
        }
    }
    println!();
    for (_, l) in c.report() {
        println!("{}", l);
    }
    for w in &c.warnings {
        println!("warning: {}", w);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut print = false;
    let mut file: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mgrade {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-p" | "--print" => print = true,
            "--new" => {
                let Some(f) = it.next() else { usage() };
                if Path::new(f).exists() {
                    eprintln!("mgrade: {} already exists", f);
                    std::process::exit(1);
                }
                if let Err(e) = std::fs::write(f, STARTER) {
                    eprintln!("mgrade: {}: {}", f, e);
                    std::process::exit(1);
                }
                println!("wrote {}", f);
                return;
            }
            s if s.starts_with('-') => {
                eprintln!("mgrade: unknown option {} (try --help)", s);
                std::process::exit(2);
            }
            _ => file = Some(a.clone()),
        }
    }
    let path = file.map(PathBuf::from).unwrap_or_else(choose);
    let course = load(&path);
    if print || !term::is_tty() {
        print_plain(&course);
        return;
    }

    term::install_panic_hook("mgrade");
    let (w, h) = term::size();
    let mut app = app::App::new(w, h, path, course);
    mtui::theme::init("mgrade");
    if let Err(e) = term::enable_raw() {
        eprintln!("mgrade: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new();
    loop {
        app.tick();
        app.render();
        if let Some(k) = input.next_key(500) {
            app.handle_key(k);
            let mut n = 0;
            while !app.quit && n < 4096 && input.ready() {
                match input.next_key(0) {
                    Some(k) => app.handle_key(k),
                    None => break,
                }
                n += 1;
            }
        }
        if term::RESIZED.swap(false, Ordering::Relaxed) {
            let (w, h) = term::size();
            app.screen.resize(w, h);
        }
        if mtui::theme::poll() {
            app.screen.invalidate();
        }
        if app.edit_file {
            app.edit_file = false;
            app.run_editor();
        }
        if app.quit {
            break;
        }
    }
    term::disable_raw();
}
