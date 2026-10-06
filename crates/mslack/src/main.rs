mod app;
mod format;
mod net;
mod ws;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

fn usage() -> ! {
    println!(
        "usage: mslack [CHANNEL]

Token: $SLACK_TOKEN, or a line `token xoxp-…` in ~/.config/mslack/config.
Live updates use Socket Mode if you add an app-level token ($SLACK_APP_TOKEN
or `apptoken xapp-…`), otherwise RTM when the token allows it (browser-session
and classic tokens). If neither works, mslack polls.
Browser-session tokens (xoxc-…) also need the `d` cookie: $SLACK_COOKIE or
`cookie xoxd-…` in the config file. See the README for how to get a token.

{}",
        app::HELP
    );
    std::process::exit(0);
}

/// Accept the `d` cookie however it was copied: with or without a `d=`
/// prefix, quotes or a trailing `;`, and URL-encoded or not.
fn normalize_cookie(c: &str) -> String {
    let c = c.trim().trim_start_matches("d=").trim_matches(['"', '\'', ';', ' ']);
    if c.contains('%') {
        c.to_string()
    } else {
        mhttp::urlencode(c)
    }
}

struct Creds {
    token: Option<String>,
    cookie: Option<String>,
    app: Option<String>,
    mouse: bool,
    images: bool,
}

/// Tokens from the environment, falling back to the config file.
fn credentials() -> Creds {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let (mut token, mut cookie, mut app) = (env("SLACK_TOKEN"), env("SLACK_COOKIE"), env("SLACK_APP_TOKEN"));
    let mut mouse = true;
    let mut images = true;
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|d| d.join("mslack/config"));
    if let Some(s) = cfg.and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in s.lines() {
            match l.trim().split_once(char::is_whitespace) {
                Some(("token", v)) if token.is_none() => token = Some(v.trim().to_string()),
                Some(("cookie", v)) if cookie.is_none() => cookie = Some(v.trim().to_string()),
                Some(("apptoken", v)) if app.is_none() => app = Some(v.trim().to_string()),
                Some(("mouse", v)) => mouse = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                Some(("images", v)) => images = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                _ => {}
            }
        }
    }
    Creds { token, cookie: cookie.map(|c| normalize_cookie(&c)).filter(|c| !c.is_empty()), app, mouse, images }
}

fn main() {
    let mut want = None;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mslack {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => want = Some(a),
        }
    }
    let Creds { token: Some(token), cookie, app, mouse, images } = credentials() else {
        eprintln!("mslack: no token; set SLACK_TOKEN or add `token …` to ~/.config/mslack/config (mslack --help)");
        std::process::exit(1);
    };
    if token.starts_with("xoxc-") && cookie.is_none() {
        eprintln!("mslack: an xoxc- token also needs the browser's `d` cookie (xoxd-…): set SLACK_COOKIE or add `cookie …` to ~/.config/mslack/config.\nIt's HttpOnly, so copy it from DevTools → Application (Firefox: Storage) → Cookies → app.slack.com → d.");
        std::process::exit(1);
    }
    if !term::is_tty() {
        eprintln!("mslack: stdin/stdout must be a terminal");
        std::process::exit(1);
    }
    term::install_panic_hook("mslack");

    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    // Live events: Socket Mode with an app token, otherwise try RTM with the
    // user token (works for browser-session and classic tokens).
    let auth = match app {
        Some(t) => ws::Auth::App(t),
        None => ws::Auth::Rtm(token.clone(), cookie.clone()),
    };
    ws::Socket { auth, base: net::base_url(), tx: tx.clone(), waker }.spawn();
    let net = net::Net::new(token, cookie, tx, waker);
    let (w, h) = term::size();
    let mut app = app::App::new(w, h, net, want, images);

    term::set_mouse(mouse);
    if let Err(e) = term::enable_raw() {
        eprintln!("mslack: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new().with_waker(waker);
    loop {
        while let Ok(r) = rx.try_recv() {
            app.on_reply(r);
        }
        let timeout = app.tick();
        app.render();
        if let Some(k) = input.next_key(timeout) {
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

#[cfg(test)]
mod tests {
    #[test]
    fn cookie_forms() {
        let enc = "xoxd-abc%2Fdef%2B%3D";
        assert_eq!(super::normalize_cookie(enc), enc);
        assert_eq!(super::normalize_cookie("xoxd-abc/def+="), enc);
        assert_eq!(super::normalize_cookie(" d=xoxd-abc%2Fdef%2B%3D; "), enc);
    }
}
