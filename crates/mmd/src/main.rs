use mmd::Options;
use std::io::Read;
use std::path::PathBuf;

fn usage() -> ! {
    println!(
        "usage: mmd [options] FILE.md      preview in the browser, reloading as the file changes
       mmd -o OUT.html FILE.md           write a standalone, printable HTML file (- for stdout)
       mmd - [-o OUT]                    read markdown from stdin

options:
  -o FILE        write HTML to FILE and exit
  -p PORT        preview port (default: any free one)
  --no-open      don't open the browser
  --embed        embed local images in the HTML (-o), so the file stands alone
  --breaks       a single newline in a paragraph is a line break
  -h, --help     -v, --version

The preview serves the file's directory, so relative images work. Press
double-click on a block to report its source line; an editor can follow it.
Print from the browser (Ctrl-P): the print style drops the chrome, keeps
code and tables together and writes link addresses out in full."
    );
    std::process::exit(0);
}

fn main() {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let mut port = 0u16;
    let (mut open, mut embed, mut breaks) = (true, false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => usage(),
            "-v" | "--version" => {
                println!("mmd {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-o" => out = args.next(),
            "-p" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            "--no-open" => open = false,
            "--embed" => embed = true,
            "--breaks" => breaks = true,
            s if s.starts_with('-') && s != "-" => {
                eprintln!("mmd: unknown option {} (try --help)", s);
                std::process::exit(2);
            }
            _ => file = Some(a),
        }
    }
    let Some(file) = file else { usage() };
    let dir = if file == "-" { None } else { Some(PathBuf::from(&file).parent().map(|p| if p.as_os_str().is_empty() { PathBuf::from(".") } else { p.to_path_buf() }).unwrap_or_else(|| PathBuf::from("."))) };
    let opts = Options { hard_breaks: breaks, embed_from: if embed { dir.clone() } else { None } };

    if out.is_some() || file == "-" {
        let src = if file == "-" {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            s
        } else {
            read(&file)
        };
        let html = mmd::to_html(&src, None, &opts);
        match out.as_deref() {
            None | Some("-") => print!("{}", html),
            Some(p) => {
                if let Err(e) = std::fs::write(p, html) {
                    eprintln!("mmd: {}: {}", p, e);
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    let title = PathBuf::from(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let server = match mmd::Server::start(port, dir, &title, opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mmd: cannot listen: {}", e);
            std::process::exit(1);
        }
    };
    read(&file);
    server.watch(PathBuf::from(&file));
    println!("mmd: {} (live; Ctrl-C to stop)", server.url());
    if open {
        if let Err(e) = mmd::open_browser(&server.url()) {
            eprintln!("mmd: {}", e);
        }
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn read(p: &str) -> String {
    match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mmd: {}: {}", p, e);
            std::process::exit(1);
        }
    }
}
