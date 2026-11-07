mod app;
mod diff;
mod notes;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

fn usage() -> ! {
    println!(
        "usage: mdiff [-b REF] [-n FILE] [-x FILE] [DIR]   watch the changes in DIR's git tree (default .)
       mdiff -p [-b REF] [DIR]                   print the changed files and exit

-b, --base REF    the branch (or commit) this one came off; the diff is from where
                  HEAD forked from it, recomputed as you go, so merging REF in later
                  doesn't bring its changes into view. Default: the branch the
                  current one was created from, else the remote's default branch
-n, --notes FILE  list FILE and the code it refers to (path:line, path:a-b) under
                  the changes
-x, --why FILE    show the newest `## ` entry of FILE (an explanation log, newest
                  first) under the diff
    --no-mouse    leave the mouse to the terminal

In a tree with .claude/pair/ (see `pair`), its BASE, NOTES.md and EXPLANATION.md
are the defaults for -b, -n and -x.

The view updates within a second of any change, untracked files included, and
jumps to it. Enter edits the file at the cursor in an mvi pane; ? shows every key."
    );
    std::process::exit(0);
}

fn die(s: impl std::fmt::Display) -> ! {
    eprintln!("mdiff: {}", s);
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut base, mut why, mut notes, mut dir, mut print, mut mouse) = (None, None, None, None, false, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mdiff {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-b" | "--base" => base = Some(it.next().cloned().unwrap_or_else(|| usage())),
            "-x" | "--why" => why = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "-n" | "--notes" => notes = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "-p" | "--print" => print = true,
            "--no-mouse" => mouse = false,
            s if s.starts_with('-') => {
                eprintln!("mdiff: unknown option {} (try --help)", s);
                std::process::exit(2);
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    let root = PathBuf::from(diff::git(&dir, &["rev-parse", "--show-toplevel"]).unwrap_or_else(|e| die(e)).trim());
    // a pair session's files, unless given
    let pair = root.join(".claude/pair");
    let found = |name: &str| Some(pair.join(name)).filter(|p| p.is_file());
    let base = base.or_else(|| std::fs::read_to_string(pair.join("BASE")).ok().map(|s| s.trim().to_string())).unwrap_or_else(|| diff::parent_ref(&root));
    let why = why.or_else(|| found("EXPLANATION.md"));
    let notes = notes.or_else(|| found("NOTES.md"));
    let fork = diff::fork_point(&root, &base).unwrap_or_else(|e| die(format!("{}: {}", base, e)));
    let label = if base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit()) { base[..8].to_string() } else { base.clone() };

    if print || !term::is_tty() {
        for f in diff::snapshot(&root, &fork).unwrap_or_else(|e| die(e)) {
            println!("{} {:>5} {:>5}  {}", f.status, format!("+{}", f.add), format!("-{}", f.del), f.name);
        }
        return;
    }

    term::install_panic_hook("mdiff");
    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    {
        let (root, base) = (root.clone(), base.clone());
        std::thread::spawn(move || {
            let mut last = None;
            loop {
                // the fork point moves when the parent branch is merged in
                let r = diff::fork_point(&root, &base).and_then(|fork| diff::snapshot(&root, &fork).map(|f| (fork, f)));
                if last.as_ref() != Some(&r) {
                    last = Some(r.clone());
                    if tx.send(r).is_err() {
                        return;
                    }
                    waker.wake();
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }

    let (w, h) = term::size();
    let mut app = app::App::new(w, h, root, label, why, notes);
    term::set_mouse(mouse);
    mtui::theme::init("mdiff");
    if let Err(e) = term::enable_raw() {
        die(format!("cannot enter raw mode: {}", e));
    }
    let mut input = term::Input::new().with_waker(waker);
    loop {
        while let Ok(r) = rx.try_recv() {
            app.on_snapshot(r);
        }
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
        if app.suspend {
            app.suspend = false;
            term::suspend();
            let (w, h) = term::size();
            app.screen.resize(w, h);
        }
        if app.quit {
            break;
        }
    }
    term::disable_raw();
}
