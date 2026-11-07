mod app;
mod diff;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

fn usage() -> ! {
    println!(
        "usage: mdiff [-b REF] [-x FILE] [DIR]   watch the changes in DIR's git tree (default .)
       mdiff -p [-b REF] [DIR]          print the changed files and exit

-b, --base REF    compare against REF instead of HEAD; commits made after it stay
                  in the diff, so `-b $(git rev-parse HEAD)` shows a whole session
-x, --notes FILE  show the newest `## ` entry of FILE (an explanation log)
    --no-mouse    leave the mouse to the terminal

The diff updates within a second of any change, untracked files included, and
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
    let (mut base, mut notes, mut dir, mut print, mut mouse) = (None, None, None, false, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mdiff {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-b" | "--base" => base = Some(it.next().cloned().unwrap_or_else(|| usage())),
            "-x" | "--notes" => notes = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
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
    let base = base.unwrap_or_else(|| "HEAD".into());
    let sha = diff::git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", base)]).unwrap_or_else(|e| die(format!("{}: {}", base, e)));
    let sha = sha.trim().to_string();
    let label = if base.len() == 40 && base.chars().all(|c| c.is_ascii_hexdigit()) { base[..8].to_string() } else { base };

    if print || !term::is_tty() {
        for f in diff::snapshot(&root, &sha).unwrap_or_else(|e| die(e)) {
            println!("{} {:>5} {:>5}  {}", f.status, format!("+{}", f.add), format!("-{}", f.del), f.name);
        }
        return;
    }

    term::install_panic_hook("mdiff");
    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    {
        let (root, sha) = (root.clone(), sha.clone());
        std::thread::spawn(move || {
            let mut last = None;
            loop {
                let r = diff::snapshot(&root, &sha);
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
    let mut app = app::App::new(w, h, root, label, notes);
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
