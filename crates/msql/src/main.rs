mod app;
mod backend;
mod config;
mod d1;
mod schema;
mod sql;
mod sqlite;

use mtui::term;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;

fn usage() -> ! {
    println!(
        "usage: msql d1 [DATABASE]
       msql sqlite FILE        a local SQLite file (needs the sqlite3 command)

Write SQL in mvi with completion from the database's schema, save queries and
run them. The first backend is Cloudflare D1.

DATABASE is a name or id. Without it msql uses the only database in the nearest
wrangler.toml / wrangler.json, or the only one in the account.

Credentials (first found wins):
  $CLOUDFLARE_API_TOKEN       an API token with the 'D1 Edit' permission
  $CLOUDFLARE_ACCOUNT_ID      the account (also account_id in wrangler.toml, or the only account the token sees)
  ~/.config/msql/config       lines `token …`, `account …`, `d1 NAME`
  wrangler login              its OAuth token is used when nothing else is set (it expires within hours)

Queries are saved in ~/.local/share/msql/<database>/queries/*.sql.

{}",
        app::HELP
    );
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("msql {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let (kind, rest) = match args.first().map(String::as_str) {
        Some("d1") => ("d1", &args[1..]),
        Some("sqlite") => ("sqlite", &args[1..]),
        None => ("d1", &args[..]),
        Some(other) => {
            eprintln!("msql: unknown backend {} (supported: d1, sqlite)", other);
            std::process::exit(1);
        }
    };
    let backend: Arc<dyn backend::Backend> = match kind {
        "sqlite" => match rest.first() {
            Some(p) => Arc::new(sqlite::Sqlite { path: p.clone() }),
            None => {
                eprintln!("usage: msql sqlite FILE");
                std::process::exit(1);
            }
        },
        _ => match d1::D1::connect(rest.first().cloned()) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                eprintln!("msql: {}", e);
                std::process::exit(1);
            }
        },
    };
    if !term::is_tty() {
        eprintln!("msql: stdin/stdout must be a terminal");
        std::process::exit(1);
    }
    term::install_panic_hook("msql");

    let waker = term::Waker::new();
    let (tx, rx) = mpsc::channel();
    let (w, h) = term::size();
    let mut app = app::App::new(w, h, backend, tx, waker);

    mtui::theme::init("msql");
    term::set_mouse(true);
    if let Err(e) = term::enable_raw() {
        eprintln!("msql: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new().with_waker(waker);
    loop {
        while let Ok(ev) = rx.try_recv() {
            app.on_event(ev);
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
