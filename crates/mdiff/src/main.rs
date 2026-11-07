mod app;
mod diff;
mod notes;
mod watch;

use diff::Repo;
use mtui::term;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn usage() -> ! {
    println!(
        "usage: mdiff [-b REF] [-n FILE] [-x FILE] [DIR]   watch the changes in DIR (default .)
       mdiff -p [-b REF] [DIR]                   print the changed files and exit

DIR is a git checkout, or a directory of them (checkouts in it and one level
down, symlinks not followed): each is shown against its own parent branch.

-b, --base REF    the branch (or commit) this one came off; the diff is from where
                  HEAD forked from it, recomputed as you go, so merging REF in later
                  doesn't bring its changes into view. Default: the branch the
                  current one was created from, else the remote's default branch
-n, --notes FILE  list FILE and the code it refers to (path:line, path:a-b) under
                  the changes
-x, --why FILE    show the newest `## ` entry of FILE (an explanation log, newest
                  first) under the diff
    --no-mouse    leave the mouse to the terminal

In DIR's .claude/pair/ (see `pair`), NOTES.md and EXPLANATION.md are the defaults
for -n and -x, and BASE for -b: one REF, or lines of `REPO-DIR REF`. In a ticket
tree made by `wt`, .wt.json gives each repo's base.

The view updates as soon as anything changes, untracked files included, and
jumps to it. Enter edits the file at the cursor in an mvi pane; ? shows every key."
    );
    std::process::exit(0);
}

fn die(s: impl std::fmt::Display) -> ! {
    eprintln!("mdiff: {}", s);
    std::process::exit(1);
}

/// Each repo's parent: -b, else BASE (`REF` for all, `DIR REF` for one),
/// else the base `wt` recorded, else what git recorded.
fn parents(top: &Path, dirs: &[String], given: Option<String>) -> Vec<Repo> {
    let base_file = std::fs::read_to_string(top.join(".claude/pair/BASE")).unwrap_or_default();
    let mut all = None;
    let mut each = std::collections::HashMap::new();
    for l in base_file.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        match l.split_once(char::is_whitespace) {
            Some((d, r)) => _ = each.insert(d.trim_end_matches('/').to_string(), r.trim().to_string()),
            None => all = Some(l.to_string()),
        }
    }
    let wt = std::fs::read_to_string(top.join(".wt.json")).ok().and_then(|s| mtui::json::parse(&s).ok());
    dirs.iter()
        .map(|d| {
            let root = if d.is_empty() { top.to_path_buf() } else { top.join(d) };
            let parent = given
                .clone()
                .or_else(|| each.get(d).cloned())
                .or_else(|| all.clone())
                .or_else(|| wt.as_ref().and_then(|v| v.get("repos").get(d).get("base").opt_str().map(String::from)))
                .unwrap_or_else(|| diff::parent_ref(&root));
            Repo { dir: d.clone(), root, parent }
        })
        .collect()
}

/// Watch every repo with inotify. Returns the watch (if there is one), the
/// repos it couldn't cover, and a line for the status bar when that's any.
fn watch_repos(repos: &[Repo]) -> (Option<watch::Watch>, HashSet<usize>, Option<String>) {
    let mut w = match watch::Watch::new() {
        Ok(w) => w,
        Err(e) => return (None, (0..repos.len()).collect(), Some(format!("no inotify ({}): polling for changes, so updates are slower", e))),
    };
    let polled: HashSet<usize> = (0..repos.len()).filter(|&i| !(w.add_tree(i, &repos[i].root) && diff::git_dir(&repos[i].root).is_none_or(|g| w.add(i, &g)))).collect();
    let note = (!polled.is_empty()).then(|| format!("out of inotify watches: polling {} repo(s) for changes", polled.len()));
    (Some(w), polled, note)
}

/// Refresh repos as they change: inotify says which, git runs only for those.
/// Repos it can't watch are polled, at a pace that keeps git's share of the
/// CPU small; every repo is re-read every few minutes regardless.
fn watcher(repos: Vec<Repo>, mut w: Option<watch::Watch>, polled: HashSet<usize>, tx: mpsc::Sender<(Vec<diff::Head>, Vec<diff::File>)>, waker: term::Waker) {
    let mut took = Duration::ZERO;
    let mut state: Vec<(diff::Head, Vec<diff::File>)> = repos.iter().enumerate().map(|(i, r)| diff::snapshot_repo(i, r)).collect();
    let mut sent = None;
    let mut full = Instant::now();
    loop {
        let snap: (Vec<diff::Head>, Vec<diff::File>) = (state.iter().map(|s| s.0.clone()).collect(), state.iter().flat_map(|s| s.1.clone()).collect());
        if sent.as_ref() != Some(&snap) {
            if tx.send(snap.clone()).is_err() {
                return;
            }
            sent = Some(snap);
            waker.wake();
        }
        let wait = if polled.is_empty() { 60_000 } else { (took.as_millis() as i32 * 4).max(3_000) };
        let mut dirty = match &mut w {
            Some(w) => {
                let mut d = w.wait(wait);
                // let a burst of writes (a save, a checkout) finish first
                let t = Instant::now();
                while !d.is_empty() && t.elapsed() < Duration::from_secs(1) {
                    let more = w.wait(150);
                    if more.is_empty() {
                        break;
                    }
                    d.extend(more);
                }
                d
            }
            None => {
                std::thread::sleep(Duration::from_millis(wait as u64));
                HashSet::new()
            }
        };
        dirty.extend(polled.iter().copied());
        if full.elapsed() > Duration::from_secs(300) {
            full = Instant::now();
            dirty.extend(0..repos.len());
        }
        let t = Instant::now();
        for i in dirty {
            state[i] = diff::snapshot_repo(i, &repos[i]);
        }
        took = t.elapsed();
    }
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
    // one checkout, or a directory of them
    let (top, dirs) = match diff::git(&dir, &["rev-parse", "--show-toplevel"]) {
        Ok(t) => (PathBuf::from(t.trim()), vec![String::new()]),
        Err(e) => {
            let top = std::fs::canonicalize(&dir).unwrap_or_else(|_| die(e.clone()));
            let found = diff::find_repos(&top);
            if found.is_empty() {
                die(format!("{}: not a git checkout, and none in it", dir.display()));
            }
            (top, found)
        }
    };
    let pair = top.join(".claude/pair");
    let found = |name: &str| Some(pair.join(name)).filter(|p| p.is_file());
    let why = why.or_else(|| found("EXPLANATION.md"));
    let notes = notes.or_else(|| found("NOTES.md"));
    let repos = parents(&top, &dirs, base);
    if repos.len() == 1 {
        if let Err(e) = diff::fork_point(&repos[0].root, &repos[0].parent) {
            die(format!("{}: {}", repos[0].parent, e));
        }
    }

    if print || !term::is_tty() {
        let (heads, files) = diff::snapshot_all(&repos);
        for h in heads {
            if let Err(e) = h.fork {
                eprintln!("mdiff: {}: {}", if h.dir.is_empty() { "." } else { &h.dir }, e);
            }
        }
        for f in files {
            println!("{} {:>5} {:>5}  {}", f.status, format!("+{}", f.add), format!("-{}", f.del), f.name);
        }
        return;
    }

    term::install_panic_hook("mdiff");
    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    let (w, polled, note) = watch_repos(&repos);
    {
        let repos = repos.clone();
        std::thread::spawn(move || watcher(repos, w, polled, tx, waker));
    }

    let (w, h) = term::size();
    let mut app = app::App::new(w, h, top, repos.len() > 1, why, notes);
    if let Some(n) = note {
        app.notice(n);
    }
    term::set_mouse(mouse);
    mtui::theme::init("mdiff");
    if let Err(e) = term::enable_raw() {
        die(format!("cannot enter raw mode: {}", e));
    }
    let mut input = term::Input::new().with_waker(waker);
    loop {
        while let Ok(r) = rx.try_recv() {
            app.on_snapshot(r.0, r.1);
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
