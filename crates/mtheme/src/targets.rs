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
    Firefox,
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
    if !firefox_profiles().is_empty() {
        v.push(Target { key: "firefox", kind: Kind::Firefox });
    }
    v
}

pub fn find(key: &str) -> Option<Target> {
    all().into_iter().find(|t| t.key == key).or_else(|| match key {
        "kitty" => Some(Target { key: "kitty", kind: Kind::Kitty }),
        "ghostty" => Some(Target { key: "ghostty", kind: Kind::Ghostty }),
        "alacritty" => Some(Target { key: "alacritty", kind: Kind::Alacritty }),
        "firefox" => Some(Target { key: "firefox", kind: Kind::Firefox }),
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

// ---- Firefox: userChrome.css / userContent.css in each install's profile ----

fn firefox_roots() -> Vec<PathBuf> {
    vec![home().join(".mozilla/firefox"), config_dir().join("mozilla/firefox"), home().join(".var/app/org.mozilla.firefox/.mozilla/firefox"), home().join("snap/firefox/common/.mozilla/firefox")]
}

/// The profile each Firefox install uses (plus any marked default).
pub fn firefox_profiles() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in firefox_roots() {
        let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else { continue };
        let (mut section, mut path, mut rel, mut default) = (String::new(), String::new(), true, false);
        let mut found: Vec<PathBuf> = Vec::new();
        let flush = |section: &str, path: &str, rel: bool, default: bool, found: &mut Vec<PathBuf>| {
            if path.is_empty() || !(section.starts_with("Install") || default) {
                return;
            }
            let p = if rel || !path.starts_with('/') { root.join(path) } else { PathBuf::from(path) };
            if p.is_dir() && !found.contains(&p) {
                found.push(p);
            }
        };
        for l in ini.lines().map(str::trim) {
            if let Some(name) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                flush(&section, &path, rel, default, &mut found);
                (section, path, rel, default) = (name.to_string(), String::new(), true, false);
            } else if let Some((k, v)) = l.split_once('=') {
                match k {
                    "Path" if !section.starts_with("Install") => path = v.to_string(),
                    "Default" if section.starts_with("Install") => path = v.to_string(),
                    "Default" => default = v == "1",
                    "IsRelative" => rel = v == "1",
                    _ => {}
                }
            }
        }
        flush(&section, &path, rel, default, &mut found);
        for p in found {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

const FF_PREF: &str = "toolkit.legacyUserProfileCustomizations.stylesheets";

fn firefox_chrome_css(t: &ThemeDef) -> String {
    let mut s = format!("/* {} — written by mtheme */\n", t.name);
    let Some(p) = &t.pal else { return s };
    let c = |x: u32| hex(rgb_of(x));
    let (base, mantle, crust, s0, s1, s2, text, sub, blue) = (c(p.base), c(p.mantle), c(p.crust), c(p.surface0), c(p.surface1), c(p.surface2), c(p.text), c(p.subtext0), c(p.blue));
    s.push_str(&format!(
        ":root {{\n  --lwt-accent-color: {mantle} !important;\n  --lwt-text-color: {text} !important;\n  --toolbar-bgcolor: {base} !important;\n  --toolbar-color: {text} !important;\n  --toolbar-field-background-color: {s0} !important;\n  --toolbar-field-color: {text} !important;\n  --toolbar-field-border-color: {s1} !important;\n  --toolbar-field-focus-background-color: {s1} !important;\n  --toolbar-field-focus-color: {text} !important;\n  --toolbar-field-focus-border-color: {blue} !important;\n  --tab-selected-bgcolor: {base} !important;\n  --tab-selected-textcolor: {text} !important;\n  --tab-loading-fill: {blue} !important;\n  --arrowpanel-background: {mantle} !important;\n  --arrowpanel-color: {text} !important;\n  --arrowpanel-border-color: {s1} !important;\n  --arrowpanel-dimmed: {s0} !important;\n  --sidebar-background-color: {mantle} !important;\n  --sidebar-text-color: {text} !important;\n  --sidebar-border-color: {s1} !important;\n  --urlbar-box-bgcolor: {s0} !important;\n  --urlbar-box-hover-bgcolor: {s1} !important;\n  --urlbar-box-active-bgcolor: {s2} !important;\n  --urlbar-box-text-color: {text} !important;\n  --toolbarbutton-icon-fill: {text} !important;\n  --toolbarbutton-hover-background: {s1} !important;\n  --toolbarbutton-active-background: {s2} !important;\n  --focus-outline-color: {blue} !important;\n  --button-primary-bgcolor: {blue} !important;\n  --button-primary-color: {base} !important;\n  --chrome-content-separator-color: {crust} !important;\n  --lwt-tab-line-color: {blue} !important;\n  --inactive-titlebar-opacity: 1 !important;\n}}\n#navigator-toolbox {{ background: {mantle} !important; border-color: {crust} !important; }}\n.tab-label:not([selected]) {{ color: {sub} !important; }}\n"
    ));
    s
}

fn firefox_content_css(t: &ThemeDef) -> String {
    let mut s = format!("/* {} — written by mtheme */\n", t.name);
    let Some(p) = &t.pal else { return s };
    let c = |x: u32| hex(rgb_of(x));
    let (base, mantle, s0, s1, text, sub, blue, purple) = (c(p.base), c(p.mantle), c(p.surface0), c(p.surface1), c(p.text), c(p.subtext0), c(p.blue), c(p.purple));
    // only Firefox's own pages (new tab, about:*), never the sites you visit
    s.push_str(&format!(
        "@-moz-document url-prefix(\"about:\") {{\n  :root {{\n    --in-content-page-background: {base} !important;\n    --in-content-page-color: {text} !important;\n    --in-content-text-color: {text} !important;\n    --in-content-deemphasized-text: {sub} !important;\n    --in-content-box-background: {s0} !important;\n    --in-content-box-border-color: {s1} !important;\n    --in-content-border-color: {s1} !important;\n    --in-content-item-hover: {s1} !important;\n    --in-content-link-color: {blue} !important;\n    --in-content-link-color-hover: {purple} !important;\n    --in-content-primary-button-background: {blue} !important;\n    --in-content-primary-button-text-color: {base} !important;\n    --newtab-background-color: {base} !important;\n    --newtab-background-color-secondary: {s0} !important;\n    --newtab-text-primary-color: {text} !important;\n    --newtab-element-hover-color: {s1} !important;\n    --newtab-border-color: {s1} !important;\n    --newtab-wallpaper-color: {mantle} !important;\n  }}\n  body {{ background-color: {base} !important; color: {text} !important; }}\n}}\n"
    ));
    s
}

fn ff_has(path: &std::path::Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.lines().any(|l| !l.trim_start().starts_with("//") && l.contains(needle)))
}

fn firefox_missing(profile: &std::path::Path) -> Vec<&'static str> {
    let mut m = Vec::new();
    if !ff_has(&profile.join("chrome/userChrome.css"), "mtheme-chrome.css") {
        m.push("userChrome.css");
    }
    if !ff_has(&profile.join("chrome/userContent.css"), "mtheme-content.css") {
        m.push("userContent.css");
    }
    if !ff_has(&profile.join("user.js"), FF_PREF) {
        m.push("user.js");
    }
    m
}

fn file_of(t: &Target) -> PathBuf {
    match t.kind {
        Kind::Kitty => config_dir().join("kitty/current-theme.conf"),
        Kind::Ghostty => config_dir().join("ghostty/themes/mtheme"),
        Kind::Alacritty => config_dir().join("alacritty/mtheme.toml"),
        Kind::Firefox => firefox_profiles().first().map(|p| p.join("chrome/mtheme-chrome.css")).unwrap_or_default(),
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
        Kind::Firefox => {
            let n = firefox_profiles().iter().filter(|p| !firefox_missing(p).is_empty()).count();
            (n > 0).then(|| format!("Firefox ({} profile{}) is not set up to read mtheme's CSS; press I (adds an @import to userChrome.css / userContent.css and a pref to user.js)", n, if n == 1 { "" } else { "s" }))
        }
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
    if t.kind == Kind::Firefox {
        let mut done = Vec::new();
        for p in firefox_profiles() {
            for f in firefox_missing(&p) {
                let (path, line, top) = match f {
                    "userChrome.css" => (p.join("chrome/userChrome.css"), "@import url(\"mtheme-chrome.css\");".to_string(), true),
                    "userContent.css" => (p.join("chrome/userContent.css"), "@import url(\"mtheme-content.css\");".to_string(), true),
                    _ => (p.join("user.js"), format!("user_pref(\"{}\", true);", FF_PREF), false),
                };
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                // @import must come before any rule, so it goes first
                let text = if top { format!("{}\n{}", line, old) } else { format!("{}{}\n", if old.is_empty() || old.ends_with('\n') { old } else { old + "\n" }, line) };
                write(&path, &text)?;
                done.push(format!("{}/{}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), f));
            }
        }
        return Ok(if done.is_empty() { "already set up".into() } else { format!("updated {} (restart Firefox)", done.join(", ")) });
    }
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
        Kind::Firefox => {
            let profiles = firefox_profiles();
            for p in &profiles {
                write(&p.join("chrome/mtheme-chrome.css"), &firefox_chrome_css(def))?;
                write(&p.join("chrome/mtheme-content.css"), &firefox_content_css(def))?;
            }
            Ok(format!("firefox: {} ({} profile{}; restart Firefox to see it)", def.name, profiles.len(), if profiles.len() == 1 { "" } else { "s" }))
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
        let ff = firefox_chrome_css(t);
        assert!(ff.contains("--toolbar-bgcolor: #1e2528") && ff.contains("--lwt-accent-color: #191e21"));
        assert!(firefox_content_css(t).contains("about:") && !firefox_chrome_css(&theme::THEMES[0]).contains("--toolbar"));
    }
}
