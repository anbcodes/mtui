mod app;
mod compose;
mod html;
mod jmap;
mod model;
mod search;
mod timefmt;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

fn usage() -> ! {
    println!(
        "usage: mmail

A Fastmail client over JMAP, the protocol the web client uses: folders, read
and starred state, drafts and sent mail are the same mailbox, and changes made
anywhere show up here within a moment (and the other way round).

Token: a Fastmail API token with Email and Email submission access
(Settings > Privacy & Security > Integrations > New API token), as
$FASTMAIL_TOKEN or a line `token fmu1-…` in ~/.config/mmail/config.
Messages are written in the built-in mvi (your ~/.config/mvi/config applies);
`editor external` in the config, or MMAIL_EDITOR=external, uses $VISUAL / $EDITOR.
Other JMAP servers: `session https://host/.well-known/jmap` in the config, or
$MMAIL_SESSION_URL. Attachments are saved to $MMAIL_DOWNLOADS (~/Downloads).

{}",
        app::HELP
    );
    std::process::exit(0);
}

fn config_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|d| d.join(name))
}

struct Config {
    token: Option<String>,
    session: String,
    mouse: bool,
    images: bool,
    external_editor: bool,
}

fn config() -> Config {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let mut c = Config { token: env("FASTMAIL_TOKEN").or_else(|| env("JMAP_TOKEN")), session: env("MMAIL_SESSION_URL").unwrap_or_default(), mouse: true, images: true, external_editor: env("MMAIL_EDITOR").is_some_and(|v| v == "external") };
    if let Some(s) = config_dir("mmail/config").and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in s.lines() {
            match l.trim().split_once(char::is_whitespace) {
                Some(("token", v)) if c.token.is_none() => c.token = Some(v.trim().to_string()),
                Some(("session", v)) if c.session.is_empty() => c.session = v.trim().to_string(),
                Some(("mouse", v)) => c.mouse = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                Some(("editor", v)) => c.external_editor = v.trim() == "external",
                Some(("images", v)) => c.images = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                _ => {}
            }
        }
    }
    if c.session.is_empty() {
        c.session = jmap::DEFAULT_SESSION.into();
    }
    c
}

fn main() {
    if let Some(a) = std::env::args().nth(1) {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mmail {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {
                eprintln!("mmail: unexpected argument {} (try --help)", a);
                std::process::exit(2);
            }
        }
    }
    let Config { token: Some(token), session, mouse, images, external_editor } = config() else {
        eprintln!("mmail: no token; set FASTMAIL_TOKEN or add `token …` to ~/.config/mmail/config (mmail --help)");
        std::process::exit(1);
    };
    if !term::is_tty() {
        eprintln!("mmail: stdin/stdout must be a terminal");
        std::process::exit(1);
    }
    let sess = match jmap::fetch_session(&session, &token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mmail: {}", e);
            std::process::exit(1);
        }
    };
    term::install_panic_hook("mmail");

    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    let me = sess.username.clone();
    let net = jmap::Net::new(token, sess, tx, waker);
    let (w, h) = term::size();
    let mut app = app::App::new(w, h, net, me, images);
    app.external_editor = external_editor;

    term::set_mouse(mouse);
    if let Err(e) = term::enable_raw() {
        eprintln!("mmail: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new().with_waker(waker);
    loop {
        while let Ok(r) = rx.try_recv() {
            app.on_reply(r);
        }
        if app.wants_editor() {
            app.run_editor();
            let (w, h) = term::size();
            app.screen.resize(w, h);
        }
        let timeout = app.tick();
        app.render();
        if let Some(k) = input.next_key(timeout) {
            app.handle_key(k);
            let mut n = 0;
            while !app.quit && !app.wants_editor() && n < 4096 && input.ready() {
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
