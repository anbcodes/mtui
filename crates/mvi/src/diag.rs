// Compiler/linter diagnostics: run a checker in the background and parse
// its output (gcc-style `file:line:col: severity: msg`, rustc `-->` blocks,
// Python tracebacks, tsc `file(l,c)`, and "line N column M" fallbacks).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Sev {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub line: usize, // 0-based
    pub col: usize,  // 0-based char column
    pub sev: Sev,
    pub msg: String,
}

pub struct CheckResult {
    pub id: u64,
    pub diags: Vec<(PathBuf, Diag)>,
    pub cmd: String,
    pub raw: String,
    pub ok: bool,
}

pub fn in_path(prog: &str) -> bool {
    if prog.contains('/') {
        return Path::new(prog).exists();
    }
    std::env::var_os("PATH").map_or(false, |p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// Pick the first usable checker for `file`. Returns (command, cwd).
pub fn pick(checks: &[String], file: &Path) -> Option<(String, PathBuf)> {
    let abs = std::fs::canonicalize(file).ok()?;
    let dir = abs.parent()?.to_path_buf();
    for c in checks {
        let (marker, cmd) = match c.split_once('|') {
            Some((m, c)) => (Some(m.trim()), c.trim()),
            None => (None, c.trim()),
        };
        let cwd = match marker {
            Some(m) => match dir.ancestors().find(|d| d.join(m).exists()) {
                Some(d) => d.to_path_buf(),
                None => continue,
            },
            None => dir.clone(),
        };
        let prog = cmd.split_whitespace().next().unwrap_or("");
        if !in_path(prog) {
            continue;
        }
        let quoted = format!("'{}'", abs.display().to_string().replace('\'', "'\\''"));
        let cmd = cmd.replace("{file}", &quoted).replace("{dir}", &format!("'{}'", dir.display()));
        return Some((cmd, cwd));
    }
    None
}

pub fn spawn(id: u64, cmd: String, cwd: PathBuf, file: PathBuf, tx: Sender<CheckResult>) {
    std::thread::spawn(move || {
        let out = Command::new("sh").arg("-c").arg(format!("{} 2>&1", cmd)).current_dir(&cwd).stdin(Stdio::null()).output();
        let (raw, ok) = match out {
            Ok(o) => (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.success()),
            Err(e) => (e.to_string(), false),
        };
        let diags = parse(&raw, &cwd, &file);
        let _ = tx.send(CheckResult { id, diags, cmd, raw, ok });
    });
}

fn sev_of(s: &str) -> Option<Sev> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.split(|c: char| c == '[' || c == '(' || c == ' ').next().unwrap_or("");
    match s {
        "error" | "fatal error" | "fatal" => Some(Sev::Error),
        "warning" | "warn" => Some(Sev::Warning),
        "note" | "info" | "help" | "hint" => Some(Sev::Info),
        _ => None,
    }
}

fn num(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let p = p.trim().trim_start_matches("./");
    let pb = cwd.join(p);
    std::fs::canonicalize(&pb).unwrap_or(pb)
}

/// Split "msg" into (severity, rest) if it starts with a severity word.
fn split_sev(rest: &str) -> (Sev, String) {
    if let Some((head, tail)) = rest.split_once(':') {
        if let Some(sv) = sev_of(head) {
            return (sv, tail.trim().to_string());
        }
    }
    let lower = rest.trim_start().to_ascii_lowercase();
    if lower.starts_with("warning") {
        return (Sev::Warning, rest.trim().to_string());
    }
    (Sev::Error, rest.trim().to_string())
}

/// Try `path:line[:col]: rest`. The path is the last whitespace-separated
/// token before the line number (so `vet: ./a.go:1:2: x` works).
fn parse_colon(line: &str) -> Option<(String, usize, usize, String)> {
    let parts: Vec<&str> = line.split(':').collect();
    for i in 1..parts.len() {
        if let Some(ln) = num(parts[i]) {
            if parts[i].starts_with(' ') {
                continue;
            }
            let path_part = parts[..i].join(":");
            let path = path_part.rsplit(|c: char| c.is_whitespace()).next().unwrap_or("").trim_matches(|c| c == '"' || c == '\'');
            if path.is_empty() || path.starts_with('-') {
                return None;
            }
            let (col, msg_start) = match parts.get(i + 1).and_then(|p| num(p)) {
                Some(c) if !parts[i + 1].starts_with(' ') => (c, i + 2),
                _ => (1, i + 1),
            };
            let msg = parts.get(msg_start..).map(|p| p.join(":")).unwrap_or_default();
            return Some((path.to_string(), ln, col, msg));
        }
    }
    None
}

/// tsc style: `path(line,col): error TS1234: msg`
fn parse_paren(line: &str) -> Option<(String, usize, usize, String)> {
    let open = line.find('(')?;
    let close = open + line[open..].find("):")?;
    let (l, c) = line[open + 1..close].split_once(',')?;
    Some((line[..open].trim().to_string(), num(l)?, num(c)?, line[close + 2..].to_string()))
}

fn find_num_after(s: &str, word: &str) -> Option<usize> {
    let i = s.find(word)?;
    let rest = &s[i + word.len()..];
    let rest = rest.trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    num(&rest[..end])
}

pub fn parse(out: &str, cwd: &Path, file: &Path) -> Vec<(PathBuf, Diag)> {
    let mut res: Vec<(PathBuf, Diag)> = Vec::new();
    let file_abs = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut pending_hdr: Option<(Sev, String)> = None; // rustc long format
    let mut pending_loc: Option<(PathBuf, usize, usize)> = None; // python traceback
    for raw in out.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        // rustc long format: "error[E0308]: msg" then "  --> file:l:c"
        if let Some(loc) = t.strip_prefix("--> ") {
            if let (Some((sv, msg)), Some((p, l, c, _))) = (pending_hdr.take(), parse_colon(loc)) {
                res.push((resolve(cwd, &p), Diag { line: l.saturating_sub(1), col: c.saturating_sub(1), sev: sv, msg }));
            }
            continue;
        }
        // python traceback: File "x.py", line 3
        if let Some(rest) = t.strip_prefix("File \"") {
            if let Some((p, tail)) = rest.split_once('"') {
                if let Some(l) = find_num_after(tail, "line") {
                    pending_loc = Some((resolve(cwd, p), l, 1));
                }
            }
            continue;
        }
        if let Some((p, l, _)) = pending_loc.clone() {
            // e.g. "SyntaxError: invalid syntax"
            if let Some((kind, msg)) = t.split_once(": ") {
                if !kind.contains(' ') && (kind.ends_with("Error") || kind.ends_with("Exception") || kind.ends_with("Warning")) {
                    let sev = if kind.ends_with("Warning") { Sev::Warning } else { Sev::Error };
                    res.push((p, Diag { line: l.saturating_sub(1), col: 0, sev, msg: format!("{}: {}", kind, msg) }));
                    pending_loc = None;
                    continue;
                }
            }
            if t.starts_with('^') || raw.starts_with(' ') {
                continue;
            }
        }
        if let Some((p, l, c, msg)) = parse_paren(t).filter(|x| !x.0.contains(' ')).or_else(|| parse_colon(t)) {
            if msg.trim().is_empty() {
                // node --check prints "file:line" then later "SyntaxError: msg"
                pending_loc = Some((resolve(cwd, &p), l, c));
                continue;
            }
            let (sev, msg) = split_sev(&msg);
            let path = resolve(cwd, &p);
            if p.contains('.') || path.exists() {
                res.push((path, Diag { line: l.saturating_sub(1), col: c.saturating_sub(1), sev, msg }));
                continue;
            }
        }
        // bash -n: "file: line 3: syntax error ..."
        if let Some(l) = find_num_after(t, ": line ") {
            let c = find_num_after(t, "column ").unwrap_or(1);
            let msg = if c > 1 { t.to_string() } else { t.splitn(3, ": ").nth(2).unwrap_or(t).to_string() };
            res.push((file_abs.clone(), Diag { line: l.saturating_sub(1), col: c.saturating_sub(1), sev: Sev::Error, msg }));
            continue;
        }
        // rustc header
        if let Some((head, msg)) = t.split_once(": ") {
            if let Some(sv) = sev_of(head) {
                if !head.contains(' ') || head.starts_with("fatal") {
                    pending_hdr = Some((sv, msg.to_string()));
                    continue;
                }
            }
        }
        // generic fallback: "... line N column M ..." (json.tool, jq)
        if let Some(l) = find_num_after(t, "line ") {
            let c = find_num_after(t, "column ").unwrap_or(1);
            res.push((file_abs.clone(), Diag { line: l.saturating_sub(1), col: c.saturating_sub(1), sev: Sev::Error, msg: t.to_string() }));
        }
    }
    // drop "note"s that duplicate locations of errors, keep the rest
    res.sort_by(|a, b| (a.0.clone(), a.1.line, a.1.sev).cmp(&(b.0.clone(), b.1.line, b.1.sev)));
    res.dedup_by(|a, b| a.0 == b.0 && a.1.line == b.1.line && a.1.msg == b.1.msg);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats() {
        let cwd = Path::new("/nonexistent");
        let f = Path::new("/nonexistent/a.py");
        let d = parse("src/main.rs:10:5: error[E0425]: cannot find value `x`\nsrc/main.rs:3:1: warning: unused import", cwd, f);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].1.line, 2);
        assert_eq!(d[0].1.sev, Sev::Warning);
        assert_eq!(d[1].1.col, 4);
        assert!(d[1].1.msg.contains("cannot find"));
        let d = parse("error[E0425]: cannot find value `x` in this scope\n  --> src/main.rs:2:13\n   |\n", cwd, f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1.line, 1);
        let d = parse("  File \"a.py\", line 3\n    x = (\n        ^\nSyntaxError: '(' was never closed\n", cwd, f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1.line, 2);
        let d = parse("src/a.ts(3,5): error TS2322: Type 'string' is not assignable", cwd, f);
        assert_eq!(d[0].1.line, 2);
        assert_eq!(d[0].1.col, 4);
        let d = parse("x.sh: line 4: syntax error near unexpected token `fi'", cwd, f);
        assert_eq!(d[0].1.line, 3);
        let d = parse("Expecting ',' delimiter: line 3 column 5 (char 20)", cwd, f);
        assert_eq!((d[0].1.line, d[0].1.col), (2, 4));
        let d = parse("vet: ./main.go:5:2: undefined: foo", cwd, f);
        assert_eq!(d[0].1.line, 4);
        assert_eq!(d[0].1.msg, "undefined: foo");
    }
}
