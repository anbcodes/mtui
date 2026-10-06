use mvi::{editor, ex};
use mtui::term;
use std::io::Read;
use std::path::PathBuf;

fn usage() -> ! {
    println!("usage: mvi [+LINE] [FILE...]   (use - to read stdin)\n       mvi --help-keys");
    std::process::exit(0);
}

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut goto: Option<usize> = None;
    let mut stdin_text: Option<String> = None;
    for a in std::env::args().skip(1) {
        if a == "-h" || a == "--help" {
            usage();
        } else if a == "--help-keys" {
            println!("{}", ex::HELP);
            return;
        } else if a == "-v" || a == "--version" {
            println!("mvi {}", env!("CARGO_PKG_VERSION"));
            return;
        } else if a == "-" {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            stdin_text = Some(s);
            // reattach the terminal as stdin
            unsafe {
                let fd = libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY);
                if fd >= 0 {
                    libc::dup2(fd, 0);
                    libc::close(fd);
                }
            }
        } else if let Some(n) = a.strip_prefix('+') {
            goto = Some(n.parse().unwrap_or(usize::MAX));
        } else {
            files.push(PathBuf::from(a));
        }
    }
    if !term::is_tty() {
        eprintln!("mvi: stdin/stdout must be a terminal");
        std::process::exit(1);
    }

    term::install_panic_hook("mvi");

    let (w, h) = term::size();
    let mut ed = editor::Editor::new(w, h);
    mvi::load_config(&mut ed);
    if let Some(t) = stdin_text {
        let b = ed.b();
        b.lines = t.lines().map(|s| s.to_string()).collect();
        if b.lines.is_empty() {
            b.lines.push(String::new());
        }
        b.name = "[stdin]".into();
    }
    for f in &files {
        ed.open_file(f);
    }
    if !files.is_empty() {
        ed.switch_buf(0);
        if ed.bufs.len() > 1 {
            ed.info(format!("{} files", ed.bufs.len()));
        }
    }
    if let Some(n) = goto {
        let y = n.saturating_sub(1).min(ed.bb().lines.len() - 1);
        ed.set_cursor((y, 0));
    }

    if let Err(e) = term::enable_raw() {
        eprintln!("mvi: cannot enter raw mode: {}", e);
        std::process::exit(1);
    }
    let mut input = term::Input::new();
    mvi::run(&mut ed, &mut input);
    term::disable_raw();
}
