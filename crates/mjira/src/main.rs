mod adf;
mod api;
mod app;
#[allow(dead_code)]
mod markup;
mod model;
mod timefmt;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

fn usage() -> ! {
    println!(
        "usage: mjira [PROJECT]

Jira Cloud: $JIRA_URL (https://you.atlassian.net), $JIRA_EMAIL and an API token
in $JIRA_API_TOKEN (id.atlassian.com > Security > API tokens).
Jira Server / Data Center: $JIRA_URL and a personal access token in
$JIRA_API_TOKEN (no email), or an email field holding the user name and the
token field the password.
All of them can instead be lines (`url …`, `email …`, `token …`, `project …`,
`api cloud|server`) in ~/.config/mjira/config. PROJECT, $JIRA_PROJECT or the
`project` line picks the project for the project and board tabs; without
one it is the project of most of your issues.

{}",
        app::HELP
    );
    std::process::exit(0);
}

fn config_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|d| d.join(name))
}

struct Config {
    url: String,
    email: String,
    token: String,
    api: String,
    project: Option<String>,
    mouse: bool,
    images: bool,
}

fn config() -> Config {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let mut c = Config {
        url: env("JIRA_URL").unwrap_or_default(),
        email: env("JIRA_EMAIL").unwrap_or_default(),
        token: env("JIRA_API_TOKEN").or_else(|| env("JIRA_TOKEN")).unwrap_or_default(),
        api: env("JIRA_API").unwrap_or_default(),
        project: env("JIRA_PROJECT"),
        mouse: true,
        images: true,
    };
    if let Some(s) = config_dir("mjira/config").and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in s.lines() {
            let Some((k, v)) = l.trim().split_once(char::is_whitespace) else { continue };
            let v = v.trim();
            let off = matches!(v, "off" | "no" | "false" | "0");
            match k {
                "url" if c.url.is_empty() => c.url = v.into(),
                "email" if c.email.is_empty() => c.email = v.into(),
                "token" if c.token.is_empty() => c.token = v.into(),
                "api" if c.api.is_empty() => c.api = v.into(),
                "project" if c.project.is_none() => c.project = Some(v.into()),
                "mouse" => c.mouse = !off,
                "images" => c.images = !off,
                _ => {}
            }
        }
    }
    if !c.url.is_empty() && !c.url.contains("://") {
        c.url = format!("https://{}", c.url);
    }
    c
}

fn main() {
    let mut want = None;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mjira {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => want = Some(a.to_uppercase()),
        }
    }
    let c = config();
    if c.url.is_empty() || c.token.is_empty() {
        eprintln!("mjira: need a site and a token; set JIRA_URL and JIRA_API_TOKEN (and JIRA_EMAIL for Jira Cloud), or add them to ~/.config/mjira/config (mjira --help)");
        std::process::exit(1);
    }
    if !term::is_tty() {
        eprintln!("mjira: stdin/stdout must be a terminal");
        std::process::exit(1);
    }
    let flavor = match c.api.as_str() {
        "cloud" => api::Flavor::Cloud,
        "server" => api::Flavor::Server,
        _ if c.url.contains(".atlassian.net") || c.url.contains(".jira.com") => api::Flavor::Cloud,
        _ => api::Flavor::Server,
    };
    let auth = if c.email.is_empty() { format!("Bearer {}", c.token) } else { format!("Basic {}", mtui::base64::encode(format!("{}:{}", c.email, c.token).as_bytes())) };
    term::install_panic_hook("mjira");

    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    let net = api::Net::new(c.url, flavor, auth, tx, waker);
    let (w, h) = term::size();
    let mut app = app::App::new(w, h, net, want.or(c.project), c.images);

    term::set_mouse(c.mouse);
    if let Err(e) = term::enable_raw() {
        eprintln!("mjira: cannot enter raw mode: {}", e);
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
