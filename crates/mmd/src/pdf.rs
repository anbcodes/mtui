// PDF output: print the standalone page with a headless Chromium/Chrome.

use std::path::Path;
use std::process::{Command, Stdio};

const BROWSERS: &[&str] = &["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome", "brave-browser", "microsoft-edge"];

fn find_browser() -> Option<String> {
    if let Ok(b) = std::env::var("MMD_BROWSER") {
        if !b.is_empty() {
            return Some(b);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for b in BROWSERS {
        if std::env::split_paths(&path).any(|d| d.join(b).is_file()) {
            return Some(b.to_string());
        }
    }
    for app in ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium"] {
        if Path::new(app).is_file() {
            return Some(app.to_string());
        }
    }
    None
}

/// Prints a complete HTML page (from `to_html`, images embedded) to `out`.
pub fn to_pdf(html: &str, out: &Path) -> Result<(), String> {
    let browser = find_browser().ok_or("no Chromium or Chrome found (install one, or set MMD_BROWSER)")?;
    let tmp = std::env::temp_dir().join(format!("mmd-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    let page = tmp.join("page.html");
    std::fs::write(&page, html).map_err(|e| format!("{}: {}", page.display(), e))?;
    let out_abs = if out.is_absolute() { out.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(out) };
    let run = |extra: &[&str]| {
        Command::new(&browser)
            .args(["--headless", "--disable-gpu", "--no-pdf-header-footer", "--print-to-pdf-no-header", "--hide-scrollbars"])
            .args(extra)
            .arg(format!("--user-data-dir={}", tmp.join("profile").display()))
            .arg(format!("--print-to-pdf={}", out_abs.display()))
            .arg(format!("file://{}", page.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
    };
    let _ = std::fs::remove_file(&out_abs);
    let mut res = run(&[]).map_err(|e| format!("{}: {}", browser, e))?;
    if !out_abs.is_file() {
        // Some sandboxes (containers, root) refuse to start Chromium without this.
        res = run(&["--no-sandbox"]).map_err(|e| format!("{}: {}", browser, e))?;
    }
    let ok = out_abs.is_file();
    let _ = std::fs::remove_dir_all(&tmp);
    if ok {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&res.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        Err(format!("{} did not write the PDF: {}", browser, last))
    }
}
