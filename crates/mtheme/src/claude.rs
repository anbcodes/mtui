// Re-colouring Claude desktop by editing its renderer stylesheet: unpack
// app.asar into a work directory, append mtheme's `claude-app` CSS to the
// renderer's stylesheet, and pack a new archive. Nothing here writes to the
// installed app; the result is a file you put wherever you run your own copy
// of the app from. Archives are handled by `npx @electron/asar` (override with
// MTHEME_ASAR, e.g. "npx asar").

use crate::sites;
use mtui::theme;
use std::path::{Path, PathBuf};
use std::process::Command;

const INSTALLED: &str = "/usr/lib/claude-desktop/resources/app.asar";
const MARK_BEGIN: &str = "/* >>> mtheme claude-app";
const MARK_END: &str = "/* <<< mtheme claude-app */";

fn work_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_else(|| PathBuf::from("."));
    base.join("mtheme/claude")
}

fn asar(args: &[&str]) -> Result<(), String> {
    let spec = std::env::var("MTHEME_ASAR").unwrap_or_else(|_| "npx --yes @electron/asar".into());
    let mut w = spec.split_whitespace();
    let prog = w.next().ok_or("MTHEME_ASAR is empty")?;
    let st = Command::new(prog).args(w).args(args).status().map_err(|e| format!("{}: {} (is node/npx installed?)", prog, e))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("asar {} failed ({})", args.first().unwrap_or(&""), st))
    }
}

struct Opts {
    from: PathBuf,
    dir: PathBuf,
    out: PathBuf,
    force: bool,
    theme: Option<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let w = work_dir();
    let mut o = Opts { from: PathBuf::from(INSTALLED), dir: w.join("src"), out: w.join("app.asar"), force: false, theme: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |what: &str| it.next().map(PathBuf::from).ok_or(format!("{} needs a value", what));
        match a.as_str() {
            "--from" => o.from = val("--from")?,
            "--dir" => o.dir = val("--dir")?,
            "--out" => o.out = val("--out")?,
            "--force" => o.force = true,
            t if !t.starts_with('-') && o.theme.is_none() => o.theme = Some(t.to_string()),
            other => return Err(format!("unknown option {}", other)),
        }
    }
    Ok(o)
}

/// Unpack the archive (and its `.unpacked` side files) into `dir`.
fn extract(o: &Opts) -> Result<String, String> {
    if o.dir.exists() {
        if !o.force {
            return Err(format!("{} exists (use --force to replace it)", o.dir.display()));
        }
        std::fs::remove_dir_all(&o.dir).map_err(|e| e.to_string())?;
    }
    if let Some(d) = o.dir.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    asar(&["extract", &o.from.to_string_lossy(), &o.dir.to_string_lossy()])?;
    restore_modes(&unpacked_files(&o.from), &o.dir);
    Ok(format!("extracted {} to {}", o.from.display(), o.dir.display()))
}

/// Renderer stylesheets that carry Claude's colour variables.
fn stylesheets(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "node_modules") {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "css") && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("--cds-gray-0") || s.contains("--bg-000:")) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(&dir.join(".vite"), &mut v);
    if v.is_empty() {
        walk(dir, &mut v);
    }
    v.sort();
    v
}

fn strip_chunk(css: &str) -> String {
    match (css.find(MARK_BEGIN), css.find(MARK_END)) {
        (Some(a), Some(b)) if b > a => format!("{}{}", &css[..a], css[b + MARK_END.len()..].trim_start_matches('\n')),
        _ => css.to_string(),
    }
}

/// Replace the mtheme chunk at the end of each colour stylesheet with one for `theme`.
fn apply(o: &Opts) -> Result<String, String> {
    let ti = match &o.theme {
        Some(t) => theme::find(t).ok_or_else(|| format!("unknown theme '{}' (mtheme list)", t))?,
        None => theme::lookup("claude").ok_or("which theme? (mtheme claude apply evergarden-winter)")?,
    };
    let files = stylesheets(&o.dir);
    if files.is_empty() {
        return Err(format!("no renderer stylesheet with Claude's colour variables under {} (extract first)", o.dir.display()));
    }
    let site = sites::all().into_iter().find(|s| s.name == "claude-app").ok_or("no claude-app template")?;
    let body = sites::render_body(&site, &theme::THEMES[ti])?;
    let chunk = if body.is_empty() { String::new() } else { format!("{} ({}) */\n{}\n{}\n", MARK_BEGIN, theme::THEMES[ti].id, body.trim(), MARK_END) };
    for f in &files {
        let css = std::fs::read_to_string(f).map_err(|e| format!("{}: {}", f.display(), e))?;
        let mut css = strip_chunk(&css);
        if !css.ends_with('\n') {
            css.push('\n');
        }
        css.push_str(&chunk);
        std::fs::write(f, css).map_err(|e| format!("{}: {}", f.display(), e))?;
    }
    let _ = theme::save("claude", ti);
    let names: Vec<String> = files.iter().map(|f| f.strip_prefix(&o.dir).unwrap_or(f).display().to_string()).collect();
    Ok(if chunk.is_empty() { format!("removed the mtheme chunk from {}", names.join(", ")) } else { format!("{} chunk added to {}", theme::THEMES[ti].name, names.join(", ")) })
}

/// Files the original keeps outside the archive (native modules, binaries) with their modes.
fn unpacked_files(from: &Path) -> Vec<(String, u32)> {
    use std::os::unix::fs::PermissionsExt;
    let root = PathBuf::from(format!("{}.unpacked", from.display()));
    fn walk(root: &Path, d: &Path, out: &mut Vec<(String, u32)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let (Ok(rel), Ok(m)) = (p.strip_prefix(root), e.metadata()) {
                out.push((rel.display().to_string(), m.permissions().mode() & 0o7777));
            }
        }
    }
    let mut v = Vec::new();
    walk(&root, &root, &mut v);
    v.sort();
    v
}

/// asar extraction writes the unpacked files with default modes; give them the originals back
/// (the executable bit on helper binaries matters).
fn restore_modes(files: &[(String, u32)], under: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for (rel, mode) in files {
        let _ = std::fs::set_permissions(under.join(rel), std::fs::Permissions::from_mode(*mode));
    }
}

/// Pack `dir` into `out`, leaving the same files unpacked as the installed archive does.
fn pack(o: &Opts) -> Result<String, String> {
    if !o.dir.is_dir() {
        return Err(format!("{} is not there (mtheme claude extract)", o.dir.display()));
    }
    if let Some(d) = o.out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&o.out);
    let _ = std::fs::remove_dir_all(format!("{}.unpacked", o.out.display()));
    let (dir, out) = (o.dir.to_string_lossy().into_owned(), o.out.to_string_lossy().into_owned());
    let files = unpacked_files(&o.from);
    // asar's --unpack takes a glob that it matches against names; basenames are what work reliably
    // with a list, so make sure each one names a single file in the tree
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    fn count(d: &Path, c: &mut std::collections::HashMap<String, usize>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                count(&p, c);
            } else if let Some(n) = p.file_name() {
                *c.entry(n.to_string_lossy().into_owned()).or_default() += 1;
            }
        }
    }
    count(&o.dir, &mut counts);
    let mut names: Vec<String> = Vec::new();
    for (rel, _) in &files {
        let base = rel.rsplit('/').next().unwrap_or(rel).to_string();
        if counts.get(&base).copied().unwrap_or(0) != 1 {
            return Err(format!("{} is not the only file called {} in the tree; cannot keep it unpacked by name", rel, base));
        }
        names.push(base);
    }
    match names.as_slice() {
        [] => asar(&["pack", &dir, &out])?,
        [one] => asar(&["pack", &dir, &out, "--unpack", one])?,
        many => asar(&["pack", &dir, &out, "--unpack", &format!("{{{}}}", many.join(","))])?,
    }
    restore_modes(&files, Path::new(&format!("{}.unpacked", out)));
    let size = std::fs::metadata(&o.out).map(|m| m.len()).unwrap_or(0);
    Ok(format!("packed {} ({:.1} MB){}", o.out.display(), size as f64 / 1e6, if files.is_empty() { String::new() } else { format!(", {} file{} left unpacked beside it", files.len(), if files.len() == 1 { "" } else { "s" }) }))
}

pub fn run(args: &[String]) -> Result<String, String> {
    let Some(cmd) = args.first() else { return Err(USAGE.into()) };
    let o = parse(&args[1..])?;
    match cmd.as_str() {
        "extract" => extract(&o),
        "apply" => apply(&o),
        "pack" => pack(&o),
        "build" => {
            let mut msgs = Vec::new();
            if !o.dir.exists() || o.force {
                msgs.push(extract(&Opts { force: o.force, ..clone(&o) })?);
            }
            msgs.push(apply(&o)?);
            msgs.push(pack(&o)?);
            msgs.push(format!("use it by putting {} (and {}.unpacked) in place of resources/app.asar of a copy of the app you can write to", o.out.display(), o.out.display()));
            Ok(msgs.join("\n"))
        }
        _ => Err(USAGE.into()),
    }
}

fn clone(o: &Opts) -> Opts {
    Opts { from: o.from.clone(), dir: o.dir.clone(), out: o.out.clone(), force: o.force, theme: o.theme.clone() }
}

pub const USAGE: &str = "usage: mtheme claude extract [--from app.asar] [--dir DIR] [--force]
       mtheme claude apply [THEME] [--dir DIR]       append (or replace) the theme's chunk in the renderer CSS
       mtheme claude pack [--dir DIR] [--out FILE]   build a new app.asar (and FILE.unpacked)
       mtheme claude build THEME                     extract if needed, apply, pack

Works in ~/.cache/mtheme/claude/ and never touches the installed app.
Archives are handled by `npx --yes @electron/asar`; set MTHEME_ASAR to use another command.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_replaced_not_stacked() {
        let css = format!("a{{b:c}}\n{} (x) */\nold\n{}\nrest", MARK_BEGIN, MARK_END);
        assert_eq!(strip_chunk(&css), "a{b:c}\nrest");
        assert_eq!(strip_chunk("plain"), "plain");
    }
}
