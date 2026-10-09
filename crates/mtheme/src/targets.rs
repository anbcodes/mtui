// Things mtheme can theme besides the mtui apps' own screens: terminal
// emulators, by writing them a colour file and asking them to reload.

use mtui::theme::{self, hex, rgb_of, ThemeDef};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    App,
    Kitty,
    Ghostty,
    Alacritty,
}

#[derive(Clone)]
pub struct Target {
    pub key: &'static str,
    pub kind: Kind,
}

const APPS: [&str; 5] = ["mvi", "mmail", "mjira", "mslack", "mgh"];

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config"))
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

fn detected(dir: &str, bin: &str) -> bool {
    config_dir().join(dir).is_dir() || on_path(bin)
}

/// The mtui apps, then whichever terminals are installed.
pub fn all() -> Vec<Target> {
    let mut v: Vec<Target> = APPS.iter().map(|&key| Target { key, kind: Kind::App }).collect();
    if detected("kitty", "kitty") {
        v.push(Target { key: "kitty", kind: Kind::Kitty });
    }
    if detected("ghostty", "ghostty") {
        v.push(Target { key: "ghostty", kind: Kind::Ghostty });
    }
    if detected("alacritty", "alacritty") {
        v.push(Target { key: "alacritty", kind: Kind::Alacritty });
    }
    v
}

pub fn find(key: &str) -> Option<Target> {
    all().into_iter().find(|t| t.key == key).or_else(|| match key {
        "kitty" => Some(Target { key: "kitty", kind: Kind::Kitty }),
        "ghostty" => Some(Target { key: "ghostty", kind: Kind::Ghostty }),
        "alacritty" => Some(Target { key: "alacritty", kind: Kind::Alacritty }),
        _ => None,
    })
}

/// A target's saved theme. Apps fall back to the `*` default; terminals only
/// have one once mtheme has set it.
pub fn current(t: &Target) -> Option<usize> {
    match t.kind {
        Kind::App => theme::lookup(t.key).or(Some(0)),
        _ => theme::read_all().iter().find(|(k, _)| k == t.key).and_then(|(_, v)| theme::find(v)),
    }
}

fn terminal_colors(t: &ThemeDef) -> Option<[String; 8 + 16]> {
    let p = t.pal.as_ref()?;
    let h = |c: u32| hex(rgb_of(c));
    let mut v: Vec<String> = vec![h(p.text), h(p.base), h(p.base), h(p.text), h(p.surface2), h(p.text), h(p.blue), h(p.surface1)];
    v.extend(t.ansi.iter().map(|&c| h(c)));
    v.try_into().ok()
}

fn write(path: &PathBuf, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {}", path.display(), e))
}

fn quiet(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn kitty_conf(t: &ThemeDef) -> String {
    let mut s = format!("# {} — written by mtheme\n", t.name);
    if let Some(c) = terminal_colors(t) {
        let [fg, bg, cur, cur_text, sel_bg, sel_fg, url, border] = [&c[0], &c[1], &c[0], &c[1], &c[4], &c[0], &c[6], &c[7]];
        s.push_str(&format!("foreground {fg}\nbackground {bg}\ncursor {cur}\ncursor_text_color {cur_text}\nselection_background {sel_bg}\nselection_foreground {sel_fg}\nurl_color {url}\nactive_border_color {url}\ninactive_border_color {border}\n"));
        s.push_str(&format!("active_tab_foreground {bg}\nactive_tab_background {url}\ninactive_tab_foreground {fg}\ninactive_tab_background {border}\ntab_bar_background {bg}\n"));
        for (i, col) in c[8..].iter().enumerate() {
            s.push_str(&format!("color{} {}\n", i, col));
        }
    }
    s
}

fn ghostty_conf(t: &ThemeDef) -> String {
    let mut s = format!("# {} — written by mtheme\n", t.name);
    if let Some(c) = terminal_colors(t) {
        s.push_str(&format!("background = {}\nforeground = {}\ncursor-color = {}\ncursor-text = {}\nselection-background = {}\nselection-foreground = {}\n", c[1], c[0], c[2], c[3], c[4], c[5]));
        for (i, col) in c[8..].iter().enumerate() {
            s.push_str(&format!("palette = {}={}\n", i, col));
        }
    }
    s
}

fn alacritty_conf(t: &ThemeDef) -> String {
    let mut s = format!("# {} — written by mtheme\n", t.name);
    if let Some(c) = terminal_colors(t) {
        s.push_str(&format!("[colors.primary]\nbackground = \"{}\"\nforeground = \"{}\"\n\n[colors.cursor]\ncursor = \"{}\"\ntext = \"{}\"\n\n[colors.selection]\nbackground = \"{}\"\ntext = \"{}\"\n", c[1], c[0], c[2], c[3], c[4], c[5]));
        let names = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
        for (group, off) in [("normal", 8), ("bright", 16)] {
            s.push_str(&format!("\n[colors.{}]\n", group));
            for (i, n) in names.iter().enumerate() {
                s.push_str(&format!("{} = \"{}\"\n", n, c[off + i]));
            }
        }
    }
    s
}

fn file_of(t: &Target) -> PathBuf {
    match t.kind {
        Kind::Kitty => config_dir().join("kitty/current-theme.conf"),
        Kind::Ghostty => config_dir().join("ghostty/themes/mtheme"),
        Kind::Alacritty => config_dir().join("alacritty/mtheme.toml"),
        Kind::App => theme::config_path(),
    }
}

/// The line the terminal's own config needs so that it uses our file.
fn include_of(t: &Target) -> Option<(PathBuf, &'static str, &'static str)> {
    match t.kind {
        Kind::Kitty => Some((config_dir().join("kitty/kitty.conf"), "current-theme.conf", "include current-theme.conf")),
        Kind::Ghostty => Some((config_dir().join("ghostty/config"), "mtheme", "theme = mtheme")),
        _ => None,
    }
}

/// Is the terminal's config set up to read our file? `Some(how to fix it)` when not.
pub fn needs_setup(t: &Target) -> Option<String> {
    match t.kind {
        Kind::App => None,
        Kind::Alacritty => {
            let conf = config_dir().join("alacritty/alacritty.toml");
            let ok = std::fs::read_to_string(&conf).is_ok_and(|s| s.contains("mtheme.toml"));
            (!ok).then(|| format!("add to {}:  [general]  import = [\"{}\"]", conf.display(), file_of(t).display()))
        }
        _ => {
            let (conf, needle, line) = include_of(t)?;
            let ok = std::fs::read_to_string(&conf).is_ok_and(|s| s.lines().any(|l| !l.trim_start().starts_with('#') && l.contains(needle)));
            (!ok).then(|| format!("{} does not read mtheme's file; press I to add `{}`", conf.display(), line))
        }
    }
}

/// Append the include line to the terminal's config (kitty, ghostty).
pub fn install(t: &Target) -> Result<String, String> {
    let Some((conf, _, line)) = include_of(t) else { return Err(needs_setup(t).unwrap_or_else(|| "nothing to install".into())) };
    if needs_setup(t).is_none() {
        return Ok("already set up".into());
    }
    let mut s = std::fs::read_to_string(&conf).unwrap_or_default();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&format!("{}\n", line));
    write(&conf, &s)?;
    Ok(format!("added `{}` to {}", line, conf.display()))
}

/// Set `t` to theme `i`: remember it and, for terminals, write their colour
/// file and ask them to reload. Returns what happened.
pub fn apply(t: &Target, i: usize) -> Result<String, String> {
    let def = &theme::THEMES[i];
    theme::save(t.key, i).map_err(|e| format!("{}: {}", theme::config_path().display(), e))?;
    let path = file_of(t);
    match t.kind {
        Kind::App => Ok(format!("{}: {}", t.key, def.name)),
        Kind::Kitty => {
            write(&path, &kitty_conf(def))?;
            let live = quiet("kitty", &["@", "set-colors", "--all", "--configured", &path.to_string_lossy()]) || quiet("pkill", &["-USR1", "-x", "kitty"]);
            Ok(format!("kitty: {}{}", def.name, if live { "" } else { " (written; restart kitty or reload its config)" }))
        }
        Kind::Ghostty => {
            write(&path, &ghostty_conf(def))?;
            let live = quiet("pkill", &["-USR2", "-x", "ghostty"]);
            Ok(format!("ghostty: {}{}", def.name, if live { "" } else { " (written; reload its config)" }))
        }
        Kind::Alacritty => {
            write(&path, &alacritty_conf(def))?;
            Ok(format!("alacritty: {} (reloads by itself once imported)", def.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_files_have_all_sixteen_colours() {
        let t = &theme::THEMES[1];
        let k = kitty_conf(t);
        assert!(k.contains("background #1e2528") && k.contains("color15 #adc9bc") && k.contains("color0 #374145"));
        assert!(ghostty_conf(t).contains("palette = 15=#adc9bc"));
        assert!(alacritty_conf(t).contains("[colors.bright]") && alacritty_conf(t).contains("white = \"#adc9bc\""));
        assert!(!kitty_conf(&theme::THEMES[0]).contains("color0"));
    }
}
