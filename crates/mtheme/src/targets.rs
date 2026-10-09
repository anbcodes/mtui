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
    Waybar,
    Rofi,
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
    if on_path("waybar") || waybar_style().is_some() {
        v.push(Target { key: "waybar", kind: Kind::Waybar });
    }
    if on_path("rofi") || config_dir().join("rofi").is_dir() {
        v.push(Target { key: "rofi", kind: Kind::Rofi });
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
        "waybar" => Some(Target { key: "waybar", kind: Kind::Waybar }),
        "rofi" => Some(Target { key: "rofi", kind: Kind::Rofi }),
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

// ---- Waybar (the bar under sway): a stylesheet imported last by the user's own ----

/// The `-s` stylesheet of the running waybar, else ~/.config/waybar/style.css.
fn waybar_running_style() -> Option<PathBuf> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) else { continue };
        if comm.trim() != "waybar" {
            continue;
        }
        let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
        let args: Vec<String> = raw.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if let Some(i) = args.iter().position(|a| a == "-s" || a == "--style") {
            if let Some(p) = args.get(i + 1) {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

/// The stylesheet to add our `@import` to. A generated one (under /run, as
/// bars that build their CSS at startup have) is followed to the last file it
/// imports, which is the one that lives in the user's config.
pub fn waybar_style() -> Option<PathBuf> {
    let mut p = std::env::var_os("MTHEME_WAYBAR_STYLE").map(PathBuf::from).or_else(waybar_running_style).or_else(|| Some(config_dir().join("waybar/style.css")).filter(|p| p.is_file()))?;
    for _ in 0..4 {
        if !p.starts_with("/run") && !p.starts_with("/tmp") {
            return Some(p);
        }
        let text = std::fs::read_to_string(&p).ok()?;
        let next = text.lines().filter_map(|l| l.trim().strip_prefix("@import")).filter_map(|r| r.split('"').nth(1).or_else(|| r.split('\'').nth(1)).map(|s| s.trim_start_matches("file://").to_string())).filter(|s| s.starts_with('/') && !s.ends_with("mtui/waybar.css")).last()?;
        p = PathBuf::from(next);
    }
    None
}

fn rofi_file() -> PathBuf {
    config_dir().join("rofi/mtheme.rasi")
}

fn rofi_rasi(t: &ThemeDef) -> String {
    let mut s = format!("/* {} — written by mtheme */\n", t.name);
    let Some(p) = &t.pal else { return s };
    let h = |x: u32| hex(rgb_of(x));
    let light = theme::luminance(rgb_of(p.base)) > 0.4;
    let ink = |bg: u32| hex(theme::legible(if light { rgb_of(p.text) } else { rgb_of(p.base) }, rgb_of(bg)));
    s.push_str(&format!(
        "* {{\n    bg: {base}; bg-alt: {s0}; bg-sel: {s1}; fg: {text}; dim: {o1}; accent: {blue}; on-accent: {on_blue}; red: {red}; on-red: {on_red}; green: {green};\n    background-color: transparent; text-color: @fg; border-color: @accent;\n}}\n\
         window {{ background-color: @bg; border: 2px; border-color: @accent; border-radius: 0; padding: 0; }}\n\
         mainbox {{ background-color: transparent; border: 0; padding: 0; spacing: 0; }}\n\
         inputbar {{ background-color: @bg-alt; padding: 8px 12px; spacing: 8px; border: 0; children: [ prompt, textbox-prompt-colon, entry, case-indicator ]; }}\n\
         prompt {{ text-color: @accent; background-color: transparent; }}\n\
         textbox-prompt-colon {{ expand: false; str: \":\"; text-color: @dim; background-color: transparent; }}\n\
         entry {{ text-color: @fg; placeholder-color: @dim; background-color: transparent; }}\n\
         case-indicator {{ text-color: @dim; background-color: transparent; }}\n\
         message {{ background-color: @bg-alt; padding: 6px 12px; border: 0; }}\n\
         textbox {{ text-color: @fg; background-color: transparent; }}\n\
         listview {{ background-color: transparent; border: 0; padding: 4px 0; scrollbar: false; spacing: 0; }}\n\
         element {{ background-color: transparent; text-color: @fg; padding: 6px 12px; border: 0; }}\n\
         element-text, element-icon {{ background-color: transparent; text-color: inherit; }}\n\
         element normal.normal, element alternate.normal {{ background-color: transparent; text-color: @fg; }}\n\
         element normal.urgent, element alternate.urgent {{ background-color: transparent; text-color: @red; }}\n\
         element normal.active, element alternate.active {{ background-color: transparent; text-color: @green; }}\n\
         element selected.normal {{ background-color: @accent; text-color: @on-accent; }}\n\
         element selected.urgent {{ background-color: @red; text-color: @on-red; }}\n\
         element selected.active {{ background-color: @accent; text-color: @on-accent; }}\n\
         scrollbar {{ handle-color: @dim; handle-width: 4px; background-color: transparent; }}\n\
         mode-switcher {{ background-color: @bg-alt; }}\n\
         button {{ background-color: transparent; text-color: @dim; padding: 6px 12px; }}\n\
         button selected {{ background-color: @bg-sel; text-color: @fg; }}\n",
        base = h(p.base), s0 = h(p.surface0), s1 = h(p.surface1), text = h(p.text), o1 = h(p.overlay1), blue = h(p.blue), red = h(p.red), green = h(p.green), on_blue = ink(p.blue), on_red = ink(p.red)
    ));
    s
}

/// The bar's background, which the desktop background follows: the theme's
/// mantle, or for Classic whatever the user's own stylesheet gives `window#waybar`.
fn bar_background(t: &ThemeDef) -> String {
    if let Some(p) = &t.pal {
        return hex(rgb_of(p.mantle));
    }
    let css = waybar_style().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    css.split("window#waybar").nth(1).and_then(|r| r.split("background:").nth(1)).and_then(|r| r.split([';', '}']).next()).map(|c| c.trim().to_string()).filter(|c| c.starts_with('#') && c.len() == 7).unwrap_or_else(|| "#000000".into())
}

fn sway_bg_file() -> PathBuf {
    theme::config_path().with_file_name("sway-bg")
}

/// Paint sway's desktop with `color` now, and remember it for the next start.
fn set_sway_bg(color: &str) -> bool {
    let _ = write(&sway_bg_file(), &format!("{}\n", color));
    quiet("swaymsg", &["output", "*", "bg", color, "solid_color"])
}

/// The line that makes the bar's startup script repaint the desktop.
const BAR_SH_LINE: &str = r#"[ -f "${XDG_CONFIG_HOME:-$HOME/.config}/mtui/sway-bg" ] && swaymsg output '*' bg "$(cat "${XDG_CONFIG_HOME:-$HOME/.config}/mtui/sway-bg")" solid_color"#;

/// `bar.sh` next to the stylesheet's directory, when there is one (the wm setup runs it from sway).
fn bar_sh() -> Option<PathBuf> {
    waybar_style().and_then(|f| f.canonicalize().ok()).and_then(|f| f.parent().map(|d| d.join("bar.sh"))).filter(|p| p.is_file())
}

fn waybar_file() -> PathBuf {
    theme::config_path().with_file_name("waybar.css")
}

fn waybar_css(t: &ThemeDef) -> String {
    let mut s = format!("/* {} — written by mtheme; imported last by your waybar stylesheet */\n", t.name);
    let Some(p) = &t.pal else { return s };
    let c = |x: u32| rgb_of(x);
    let h = |x: u32| hex(rgb_of(x));
    let light = theme::luminance(c(p.base)) > 0.4;
    // text that reads on a coloured button
    let ink = |bg: u32| hex(theme::legible(if light { c(p.text) } else { c(p.base) }, c(bg)));
    // an accent as text on the pill background
    let on_pill = |fg: u32| hex(theme::legible(c(fg), c(p.surface0)));
    s.push_str(&format!(
        "window#waybar {{ background: {mantle}; color: {text}; }}\n\
         .ws {{ background: {s0}; color: {sub}; }}\n\
         .ws.empty {{ background: transparent; color: {o1}; }}\n\
         .ws.visible {{ background: {s2}; color: {text}; }}\n\
         .ws.focused {{ background: {blue}; color: {on_blue}; box-shadow: none; }}\n\
         .ws.urgent {{ background: {red}; color: {on_red}; }}\n\
         #mode {{ background: {red}; color: {on_red}; }}\n\
         #pulseaudio, #backlight, #network, #cpu, #memory, #disk, #battery, #clock {{ background: {s0}; color: {text}; }}\n\
         #pulseaudio.muted {{ color: {o1}; }}\n\
         #network.disconnected {{ color: {red_t}; }}\n\
         #battery.charging, #battery.plugged {{ color: {green_t}; }}\n\
         #battery.warning:not(.charging) {{ color: {yellow_t}; }}\n\
         #battery.critical:not(.charging) {{ background: {red}; color: {on_red}; }}\n\
         #workspaces button {{ color: {sub}; background: transparent; }}\n\
         #workspaces button.focused, #workspaces button.active {{ color: {on_blue}; background: {blue}; }}\n\
         #workspaces button.urgent {{ color: {on_red}; background: {red}; }}\n\
         tooltip {{ background: {s0}; color: {text}; border: 1px solid {s1}; }}\n\
         tooltip label {{ color: {text}; }}\n",
        mantle = h(p.mantle), text = h(p.text), s0 = h(p.surface0), s1 = h(p.surface1), s2 = h(p.surface2), sub = h(p.subtext0), o1 = h(p.overlay1), blue = h(p.blue), red = h(p.red),
        on_blue = ink(p.blue), on_red = ink(p.red), red_t = on_pill(p.red), green_t = on_pill(p.green), yellow_t = on_pill(p.yellow)
    ));
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
    let light = theme::luminance(rgb_of(p.base)) > 0.4;
    let ink = |bg: u32| hex(theme::legible(if light { rgb_of(p.text) } else { rgb_of(p.base) }, rgb_of(bg)));
    let (base, mantle, crust, s0, s1, s2, text, sub, o1, blue, red) = (c(p.base), c(p.mantle), c(p.crust), c(p.surface0), c(p.surface1), c(p.surface2), c(p.text), c(p.subtext0), c(p.overlay1), c(p.blue), c(p.red));
    let (on_blue, scheme) = (ink(p.blue), if light { "light" } else { "dark" });
    s.push_str(&format!(
        r#":root {{
  color-scheme: {scheme} !important;
  --lwt-accent-color: {mantle} !important;
  --lwt-text-color: {text} !important;
  --lwt-toolbar-field-background-color: {s0} !important;
  --lwt-toolbar-field-color: {text} !important;
  --toolbar-bgcolor: {base} !important;
  --toolbar-color: {text} !important;
  --toolbar-field-background-color: {s0} !important;
  --toolbar-field-color: {text} !important;
  --toolbar-field-border-color: {s1} !important;
  --toolbar-field-focus-background-color: {s1} !important;
  --toolbar-field-focus-color: {text} !important;
  --toolbar-field-focus-border-color: {blue} !important;
  --tab-selected-bgcolor: {base} !important;
  --tab-selected-textcolor: {text} !important;
  --tab-loading-fill: {blue} !important;
  --arrowpanel-background: {mantle} !important;
  --arrowpanel-color: {text} !important;
  --arrowpanel-border-color: {s1} !important;
  --arrowpanel-dimmed: {s0} !important;
  --panel-background: {mantle} !important;
  --panel-color: {text} !important;
  --panel-border-color: {s1} !important;
  --panel-description-color: {sub} !important;
  --sidebar-background-color: {mantle} !important;
  --sidebar-text-color: {text} !important;
  --sidebar-border-color: {s1} !important;
  --urlbar-box-bgcolor: {s0} !important;
  --urlbar-box-hover-bgcolor: {s1} !important;
  --urlbar-box-active-bgcolor: {s2} !important;
  --urlbar-box-text-color: {text} !important;
  --urlbar-popup-url-color: {blue} !important;
  --toolbarbutton-icon-fill: {text} !important;
  --toolbarbutton-icon-fill-attention: {blue} !important;
  --toolbarbutton-hover-background: {s1} !important;
  --toolbarbutton-active-background: {s2} !important;
  --toolbarbutton-hover-bordercolor: transparent !important;
  --focus-outline-color: {blue} !important;
  --button-primary-bgcolor: {blue} !important;
  --button-primary-color: {on_blue} !important;
  --chrome-content-separator-color: {crust} !important;
  --lwt-tab-line-color: {blue} !important;
  --inactive-titlebar-opacity: 1 !important;
  --in-content-page-background: {base} !important;
  --in-content-page-color: {text} !important;
}}

/* frame, tab strip, toolbars */
#navigator-toolbox, #titlebar, #TabsToolbar, #toolbar-menubar {{ background: {mantle} !important; color: {text} !important; border-color: {crust} !important; }}
#nav-bar, #PersonalToolbar {{ background: {base} !important; color: {text} !important; border-color: {s1} !important; }}
#nav-bar toolbarbutton, #PersonalToolbar toolbarbutton, #TabsToolbar toolbarbutton, #nav-bar .toolbarbutton-1 {{ color: {text} !important; fill: {text} !important; }}
toolbarbutton:hover > .toolbarbutton-icon, .toolbarbutton-1:hover > .toolbarbutton-icon {{ background-color: {s1} !important; }}
.bookmark-item, .bookmark-item > .toolbarbutton-text, #PersonalToolbar .toolbarbutton-text {{ color: {text} !important; }}
#PersonalToolbar #import-button, #PersonalToolbar .chromeclass-toolbar-additional {{ color: {text} !important; }}
#personal-bookmarks, #PlacesToolbarItems {{ color: {text} !important; }}

/* tabs */
.tabbrowser-tab .tab-background {{ background-color: transparent !important; border-radius: 6px; }}
.tabbrowser-tab:hover:not([selected]) .tab-background {{ background-color: {s0} !important; }}
.tabbrowser-tab[selected] .tab-background, .tab-background[selected] {{ background: {base} !important; background-color: {base} !important; box-shadow: none !important; border: 1px solid {s1} !important; }}
.tabbrowser-tab .tab-label {{ color: {sub} !important; }}
.tabbrowser-tab[selected] .tab-label {{ color: {text} !important; }}
.tab-icon-image, .tab-close-button {{ fill: {text} !important; color: {text} !important; }}
.tab-close-button:hover {{ background-color: {s2} !important; }}
#tabbrowser-arrowscrollbox ~ toolbarbutton, #new-tab-button {{ color: {text} !important; fill: {text} !important; }}

/* URL bar */
#urlbar, #searchbar {{ color: {text} !important; }}
#urlbar-background, #searchbar {{ background: {s0} !important; background-color: {s0} !important; border: 1px solid {s1} !important; box-shadow: none !important; }}
#urlbar:hover #urlbar-background {{ background: {s1} !important; }}
#urlbar[focused] #urlbar-background, #urlbar[open] #urlbar-background {{ background: {s1} !important; border-color: {blue} !important; }}
#urlbar-input, .urlbar-input, #urlbar-input::placeholder, .urlbar-input::placeholder {{ color: {text} !important; -moz-context-properties: fill !important; opacity: 1 !important; }}
#urlbar-input::placeholder, .urlbar-input::placeholder {{ color: {o1} !important; }}
#urlbar-input::selection {{ background: {blue} !important; color: {on_blue} !important; }}
#urlbar .urlbar-icon, #identity-icon, #tracking-protection-icon, #page-action-buttons .urlbar-icon {{ fill: {text} !important; color: {text} !important; }}
#identity-box, #identity-icon-label, #tracking-protection-icon-box, .urlbar-page-action {{ color: {text} !important; }}
#urlbar-label-box, #urlbar-search-mode-indicator {{ background: {s2} !important; color: {text} !important; }}
.urlbarView {{ background: {mantle} !important; color: {text} !important; }}
.urlbarView-row[selected], .urlbarView-row:hover {{ background: {s1} !important; color: {text} !important; }}
.urlbarView-title, .urlbarView-no-wrap, .urlbarView-action {{ color: {text} !important; }}
.urlbarView-url, .urlbarView-title-separator::before {{ color: {blue} !important; }}
.urlbarView-row[selected] .urlbarView-title, .urlbarView-row[selected] .urlbarView-url {{ color: {text} !important; }}
.search-one-offs, .searchbar-engine-one-off-item {{ background: {mantle} !important; color: {text} !important; }}

/* popups, menus, panels, sidebar, find bar */
menupopup, panel, .panel-arrowcontent, .panel-subview-body, #appMenu-popup {{ --panel-background: {mantle}; --panel-color: {text}; color: {text} !important; background: {mantle} !important; border-color: {s1} !important; }}
menuitem, menu, .subviewbutton, .panel-subview-footer-button {{ color: {text} !important; }}
menuitem[_moz-menuactive], menu[_moz-menuactive], .subviewbutton:hover:not([disabled]), toolbarbutton[_moz-menuactive] {{ background-color: {s1} !important; color: {text} !important; }}
menuitem[disabled], .subviewbutton[disabled] {{ color: {o1} !important; }}
menuseparator, toolbarseparator, .panel-header, .panel-subview-footer {{ border-color: {s1} !important; background-color: transparent !important; }}
#sidebar-box, #sidebar-header, #sidebar {{ background: {mantle} !important; color: {text} !important; }}
findbar {{ background: {mantle} !important; color: {text} !important; border-color: {s1} !important; }}
findbar textbox, .findbar-textbox {{ background: {s0} !important; color: {text} !important; border-color: {s1} !important; }}
#browser, #appcontent, #tabbrowser-tabpanels, #tabbrowser-tabbox {{ background: {base} !important; }}
tooltip {{ background: {mantle} !important; color: {text} !important; border-color: {s1} !important; }}
.badged-button .toolbarbutton-badge {{ background-color: {red} !important; }}
notification, .notificationbox-stack, .infobar, notification-message {{ --info-bar-background: {s1}; background: {s1} !important; color: {text} !important; border-color: {s2} !important; }}
notification .notification-button, .infobar button {{ background: {s2} !important; color: {text} !important; }}
"#
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
        "@-moz-document url-prefix(\"about:\") {{\n  :root {{\n    --in-content-page-background: {base} !important;\n    --in-content-page-color: {text} !important;\n    --in-content-text-color: {text} !important;\n    --in-content-deemphasized-text: {sub} !important;\n    --in-content-box-background: {s0} !important;\n    --in-content-box-border-color: {s1} !important;\n    --in-content-border-color: {s1} !important;\n    --in-content-item-hover: {s1} !important;\n    --in-content-link-color: {blue} !important;\n    --in-content-link-color-hover: {purple} !important;\n    --in-content-primary-button-background: {blue} !important;\n    --in-content-primary-button-text-color: {base} !important;\n    --newtab-background-color: {base} !important;\n    --newtab-background-color-secondary: {s0} !important;\n    --newtab-text-primary-color: {text} !important;\n    --newtab-element-hover-color: {s1} !important;\n    --newtab-border-color: {s1} !important;\n    --newtab-wallpaper-color: {mantle} !important;\n    --newtab-wordmark-color: {text} !important;\n    --newtab-link-primary-color: {blue} !important;\n    --newtab-primary-action-background: {blue} !important;\n    --newtab-card-background-color: {s0} !important;\n    --newtab-section-header-text-color: {text} !important;\n    --newtab-text-secondary-color: {sub} !important;\n  }}\n  body {{ background-color: {base} !important; color: {text} !important; }}\n}}\n"
    ));
    s
}

fn ff_has_css_import(path: &std::path::Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.lines().any(|l| l.contains("@import") && l.contains(needle)))
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
        Kind::Waybar => waybar_file(),
        Kind::Rofi => rofi_file(),
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
        Kind::Rofi => {
            let conf = config_dir().join("rofi/config.rasi");
            let ok = std::fs::read_to_string(&conf).is_ok_and(|s| s.lines().any(|l| !l.trim_start().starts_with("//") && l.contains("mtheme.rasi")));
            (!ok).then(|| format!("{} does not load mtheme's theme; press I to add the @import", conf.display()))
        }
        Kind::Waybar => {
            let Some(style) = waybar_style() else { return Some("could not find waybar's stylesheet; set MTHEME_WAYBAR_STYLE=/path/to/style.css".into()) };
            let ok = ff_has_css_import(&style, "mtui/waybar.css");
            let sh_ok = bar_sh().is_none_or(|b| std::fs::read_to_string(b).is_ok_and(|t| t.contains("mtui/sway-bg")));
            (!ok || !sh_ok).then(|| format!("{} / bar.sh do not load mtheme's files; press I to add the @import and the desktop-background line", style.display()))
        }
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
    if t.kind == Kind::Rofi {
        if needs_setup(t).is_none() {
            return Ok("already set up".into());
        }
        let conf = config_dir().join("rofi/config.rasi");
        let mut text = std::fs::read_to_string(&conf).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !rofi_file().exists() {
            write(&rofi_file(), &rofi_rasi(&theme::THEMES[theme::lookup("rofi").unwrap_or(0)]))?;
        }
        text.push_str(&format!("@import \"{}\"\n", rofi_file().display()));
        write(&conf, &text)?;
        return Ok(format!("added an @import of {} to {}", rofi_file().display(), conf.display()));
    }
    if t.kind == Kind::Waybar {
        let Some(style) = waybar_style() else { return Err("could not find waybar's stylesheet; set MTHEME_WAYBAR_STYLE=/path/to/style.css".into()) };
        let mut did = Vec::new();
        if let Some(b) = bar_sh() {
            let text = std::fs::read_to_string(&b).map_err(|e| format!("{}: {}", b.display(), e))?;
            if !text.contains("mtui/sway-bg") {
                // before the final `exec waybar`, which replaces the shell
                let at = text.rfind("\nexec ").map(|i| i + 1).unwrap_or(text.len());
                write(&b, &format!("{}{}\n{}", &text[..at], BAR_SH_LINE, &text[at..]))?;
                did.push(format!("the desktop-background line to {}", b.display()));
            }
        }
        if ff_has_css_import(&style, "mtui/waybar.css") {
            return Ok(if did.is_empty() { "already set up".into() } else { format!("added {}", did.join(" and ")) });
        }
        let file = waybar_file();
        if !file.exists() {
            write(&file, &waybar_css(&theme::THEMES[theme::lookup("waybar").unwrap_or(0)]))?;
        }
        let mut text = std::fs::read_to_string(&style).map_err(|e| format!("{}: {}", style.display(), e))?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        // last, so it wins over the rules above it
        text.push_str(&format!("@import url(\"{}\");\n", file.display()));
        write(&style, &text)?;
        did.insert(0, format!("an @import of {} to {}", file.display(), style.display()));
        return Ok(format!("added {}", did.join(" and ")));
    }
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
        Kind::Rofi => {
            write(&path, &rofi_rasi(def))?;
            Ok(format!("rofi: {} (used the next time rofi opens)", def.name))
        }
        Kind::Waybar => {
            write(&path, &waybar_css(def))?;
            let live = quiet("pkill", &["-USR2", "-x", "waybar"]);
            let bg = bar_background(def);
            let painted = set_sway_bg(&bg);
            Ok(format!("waybar: {}, desktop {}{}", def.name, bg, if live && painted { "" } else { " (written; restart waybar / reload sway)" }))
        }
        Kind::Firefox => {
            let profiles = firefox_profiles();
            let (site_css, site_errs) = crate::sites::render_all(def);
            for p in &profiles {
                write(&p.join("chrome/mtheme-chrome.css"), &firefox_chrome_css(def))?;
                write(&p.join("chrome/mtheme-content.css"), &format!("{}{}", firefox_content_css(def), site_css))?;
            }
            let note = if site_errs.is_empty() { String::new() } else { format!(" — site problems: {}", site_errs.join("; ")) };
            Ok(format!("firefox: {} ({} profile{}; restart Firefox to see it){}", def.name, profiles.len(), if profiles.len() == 1 { "" } else { "s" }, note))
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
        assert!(rofi_rasi(t).contains("bg: #1e2528") && rofi_rasi(&theme::THEMES[0]).lines().count() == 1);
        assert_eq!(bar_background(t), "#191e21");
        let wb = waybar_css(t);
        assert!(wb.contains("window#waybar { background: #191e21") && wb.contains(".ws.focused { background: #b2caed;"));
        // light theme: text on the blue button must still read
        let sm = waybar_css(&theme::THEMES[4]);
        let line = sm.lines().find(|l| l.starts_with(".ws.focused")).unwrap();
        let fg = line.split("color: ").last().unwrap().trim_end_matches("; }").to_string();
        let parse = |h: &str| (u8::from_str_radix(&h[1..3], 16).unwrap(), u8::from_str_radix(&h[3..5], 16).unwrap(), u8::from_str_radix(&h[5..7], 16).unwrap());
        assert!(theme::contrast(parse(&fg), rgb_of(0x8294AD)) >= 4.5, "{}", line);
        let ff = firefox_chrome_css(t);
        assert!(ff.contains("--toolbar-bgcolor: #1e2528") && ff.contains("--lwt-accent-color: #191e21"));
        assert!(firefox_content_css(t).contains("about:") && !firefox_chrome_css(&theme::THEMES[0]).contains("--toolbar"));
    }
}
