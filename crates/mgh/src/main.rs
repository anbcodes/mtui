mod api;
mod app;

use mtui::term;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

fn usage() -> ! {
    println!(
        "usage: mgh [OWNER/REPO]

Token: $GITHUB_TOKEN (or $GH_TOKEN), a line `token ghp_…` in ~/.config/mgh/config,
or the one `gh` saved in ~/.config/gh/hosts.yml. A fine-grained or classic
token with `repo` and `notifications` access covers everything mgh does.
With no argument the repo tab follows the `origin` remote of the current
directory.

{}",
        app::HELP
    );
    std::process::exit(0);
}

fn config_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|d| d.join(name))
}

struct Creds {
    token: Option<String>,
    mouse: bool,
    images: bool,
}

fn credentials() -> Creds {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let mut token = env("GITHUB_TOKEN").or_else(|| env("GH_TOKEN"));
    let mut mouse = true;
    let mut images = true;
    if let Some(s) = config_dir("mgh/config").and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in s.lines() {
            match l.trim().split_once(char::is_whitespace) {
                Some(("token", v)) if token.is_none() => token = Some(v.trim().to_string()),
                Some(("mouse", v)) => mouse = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                Some(("images", v)) => images = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                _ => {}
            }
        }
    }
    if token.is_none() {
        token = config_dir("gh/hosts.yml").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("oauth_token:").map(|t| t.trim().to_string())));
    }
    Creds { token: token.filter(|t| !t.is_empty()), mouse, images }
}

/// OWNER/REPO from a git remote URL (https or ssh form).
pub fn parse_remote(url: &str) -> Option<String> {
    let rest = url.trim().split_once("github.com")?.1;
    let rest = rest.trim_start_matches([':', '/']).trim_end_matches('/').trim_end_matches(".git");
    let mut it = rest.split('/');
    let (o, r) = (it.next()?, it.next()?);
    (!o.is_empty() && !r.is_empty() && it.next().is_none()).then(|| format!("{}/{}", o, r))
}

fn origin_repo() -> Option<String> {
    let out = std::process::Command::new("git").args(["remote", "get-url", "origin"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| parse_remote(&String::from_utf8_lossy(&out.stdout))).flatten()
}

fn main() {
    let mut want = None;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mgh {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => want = Some(a),
        }
    }
    let Creds { token: Some(token), mouse, images } = credentials() else {
        eprintln!("mgh: no token; set GITHUB_TOKEN or add `token …` to ~/.config/mgh/config (mgh --help)");
        std::process::exit(1);
    };
    if !term::is_tty() {
        eprintln!("mgh: stdin/stdout must be a terminal");
        std::process::exit(1);
    }
    term::install_panic_hook("mgh");

    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    let net = api::Net::new(token, tx, waker);
    let (w, h) = term::size();
    let explicit = want.is_some();
    let mut app = app::App::new(w, h, net, want.or_else(origin_repo), explicit, images);

    term::set_mouse(mouse);
    mtui::theme::init("mgh");
    if let Err(e) = term::enable_raw() {
        eprintln!("mgh: cannot enter raw mode: {}", e);
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
    use super::parse_remote;

    #[test]
    fn remotes() {
        assert_eq!(parse_remote("https://github.com/foo/bar.git\n").as_deref(), Some("foo/bar"));
        assert_eq!(parse_remote("git@github.com:foo/bar.git").as_deref(), Some("foo/bar"));
        assert_eq!(parse_remote("ssh://git@github.com/foo/bar").as_deref(), Some("foo/bar"));
        assert_eq!(parse_remote("https://example.com/foo"), None);
    }
}
