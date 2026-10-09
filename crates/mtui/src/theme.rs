// Shared colour themes. The apps draw with xterm-256 indices ("colour 236 for
// bars, 180 for the accent, ..."); a theme re-maps those indices to its own
// palette when the screen is written, so every app is themable without
// changing how it picks colours:
//
//   * greys (232..255 and the cube's greys) become the theme's surface /
//     overlay / text ramp, so "dark grey bar" stays a bar and "light grey
//     text" stays text;
//   * hues become the theme's nearest accent (red, orange, yellow, green,
//     blue, purple, ...), and dark shades of a hue become that accent
//     tinted into the background (diff backgrounds, selections);
//   * index 0, "the terminal default", becomes the theme's base / text.
//
// Whichever theme is active, text that would fall below 4.5:1 contrast is
// lifted or darkened toward readability.
//
// Each app remembers its own theme in `~/.config/mtui/themes` (`app=theme`
// lines, `*=theme` for a default); `MTUI_THEME` overrides it for one run.
// Running apps notice edits to that file (see `poll`), which is how
// `mtheme` switches them live.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

pub type Rgb = (u8, u8, u8);

/// A theme's named colours (the Catppuccin / Evergarden naming).
pub struct Pal {
    pub base: u32,
    pub mantle: u32,
    pub crust: u32,
    pub surface0: u32,
    pub surface1: u32,
    pub surface2: u32,
    pub overlay0: u32,
    pub overlay1: u32,
    pub overlay2: u32,
    pub subtext0: u32,
    pub subtext1: u32,
    pub text: u32,
    pub red: u32,
    pub orange: u32,
    pub yellow: u32,
    pub lime: u32,
    pub green: u32,
    pub aqua: u32,
    pub skye: u32,
    pub snow: u32,
    pub blue: u32,
    pub purple: u32,
    pub pink: u32,
}

pub struct ThemeDef {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    /// None: the classic look, plain xterm colours on the terminal's own
    /// background.
    pub pal: Option<Pal>,
    /// The 16 ANSI colours (black..white, then bright), for terminal themes.
    pub ansi: [u32; 16],
}

const EVERGARDEN_DARK_ACCENTS: (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) = (0xF8F9E8, 0xF57F82, 0xF7A182, 0xF5D098, 0xDBE6AF, 0xCBE3B3, 0xB3E3CA, 0xB3E6DB, 0xAFD9E6, 0xB2CAED, 0xD2BDF3, 0xF3C0E5);

const fn dark(base: u32, mantle: u32, crust: u32, s0: u32, s1: u32) -> Pal {
    let a = EVERGARDEN_DARK_ACCENTS;
    Pal { base, mantle, crust, surface0: s0, surface1: s1, surface2: 0x4A585C, overlay0: 0x58686D, overlay1: 0x6F8788, overlay2: 0x839E9A, subtext0: 0x96B4AA, subtext1: 0xADC9BC, text: a.0, red: a.1, orange: a.2, yellow: a.3, lime: a.4, green: a.5, aqua: a.6, skye: a.7, snow: a.8, blue: a.9, purple: a.10, pink: a.11 }
}

const fn ansi_dark(black: u32) -> [u32; 16] {
    [black, 0xF57F82, 0xCBE3B3, 0xF5D098, 0xB2CAED, 0xF3C0E5, 0xB3E3CA, 0x96B4AA, 0x6F8788, 0xF57F82, 0xCBE3B3, 0xF5D098, 0xB2CAED, 0xF3C0E5, 0xB3E3CA, 0xADC9BC]
}

pub static THEMES: [ThemeDef; 5] = [
    ThemeDef { id: "classic", name: "Classic", dark: true, pal: None, ansi: [0; 16] },
    ThemeDef { id: "evergarden-winter", name: "Evergarden Winter", dark: true, pal: Some(dark(0x1E2528, 0x191E21, 0x171C1F, 0x262F33, 0x374145)), ansi: ansi_dark(0x374145) },
    ThemeDef { id: "evergarden-fall", name: "Evergarden Fall", dark: true, pal: Some(dark(0x232A2E, 0x1C2225, 0x171C1F, 0x2B3337, 0x374145)), ansi: ansi_dark(0x374145) },
    ThemeDef { id: "evergarden-spring", name: "Evergarden Spring", dark: true, pal: Some(dark(0x2B3438, 0x232A2E, 0x1C2225, 0x343E43, 0x3E4A4F)), ansi: ansi_dark(0x3E4A4F) },
    ThemeDef {
        id: "evergarden-summer",
        name: "Evergarden Summer",
        dark: false,
        pal: Some(Pal {
            base: 0xF5EFE6,
            mantle: 0xF2EAE1,
            crust: 0xE8DED5,
            surface0: 0xEDE8DD,
            surface1: 0xE6E1D3,
            surface2: 0xCECCBD,
            overlay0: 0xACB5A4,
            overlay1: 0x829084,
            overlay2: 0x707D76,
            subtext0: 0x576869,
            subtext1: 0x455355,
            text: 0x2B3034,
            red: 0xC58687,
            orange: 0xC69883,
            yellow: 0xC4AA80,
            lime: 0xABB182,
            green: 0x91A77A,
            aqua: 0x74A48B,
            skye: 0x719F96,
            snow: 0x7799A3,
            blue: 0x8294AD,
            purple: 0xA897B8,
            pink: 0xC499B8,
        }),
        ansi: [0xE6E1D3, 0xC58687, 0x91A77A, 0xC4AA80, 0x8294AD, 0xC499B8, 0x74A48B, 0x576869, 0x829084, 0xC58687, 0x91A77A, 0xC4AA80, 0x8294AD, 0xC499B8, 0x74A48B, 0x455355],
    },
];

pub fn rgb_of(c: u32) -> Rgb {
    ((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// sRGB of an xterm-256 palette index.
pub fn xterm_rgb(i: u8) -> Rgb {
    const BASE: [Rgb; 16] = [(0, 0, 0), (205, 0, 0), (0, 205, 0), (205, 205, 0), (0, 0, 238), (205, 0, 205), (0, 205, 205), (229, 229, 229), (127, 127, 127), (255, 0, 0), (0, 255, 0), (255, 255, 0), (92, 92, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255)];
    const LV: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = (i - 16) as usize;
            (LV[n / 36], LV[n / 6 % 6], LV[n % 6])
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

/// The xterm-256 index closest to a colour (for terminals without 24-bit).
pub fn nearest(c: Rgb) -> u8 {
    let d = |a: Rgb| {
        let (dr, dg, db) = (a.0 as i32 - c.0 as i32, a.1 as i32 - c.1 as i32, a.2 as i32 - c.2 as i32);
        dr * dr + dg * dg + db * db
    };
    (16..=255u8).min_by_key(|&i| d(xterm_rgb(i))).unwrap_or(231)
}

pub fn luminance(c: Rgb) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
}

pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// `fg` if it reads on `bg` (4.5:1); otherwise `fg` with its lightness moved
/// (up on a dark background, down on a light one) until it does, keeping its
/// hue so a red stays red rather than going grey.
pub fn legible(fg: Rgb, bg: Rgb) -> Rgb {
    if contrast(fg, bg) >= 4.5 {
        return fg;
    }
    let up = contrast(bg, (255, 255, 255)) > contrast(bg, (0, 0, 0));
    let (h, s, l) = hsl(fg);
    for k in 1..=50 {
        let l2 = if up { l + (1.0 - l) * k as f32 / 50.0 } else { l * (1.0 - k as f32 / 50.0) };
        // a darker colour needs a little more saturation to still read as its hue
        let s2 = if up { s } else { (s * (1.0 + 0.5 * k as f32 / 50.0)).min(1.0) };
        let c = from_hsl(h, s2, l2);
        if contrast(c, bg) >= 4.5 {
            return c;
        }
    }
    if up {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    }
}

pub fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (f(r), f(g), f(b))
}

pub fn hsl(c: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

impl Pal {
    /// A colour by its palette name (`base`, `surface1`, `blue`, ...).
    pub fn get(&self, name: &str) -> Option<u32> {
        Some(match name {
            "base" => self.base,
            "mantle" => self.mantle,
            "crust" => self.crust,
            "surface0" => self.surface0,
            "surface1" => self.surface1,
            "surface2" => self.surface2,
            "overlay0" => self.overlay0,
            "overlay1" => self.overlay1,
            "overlay2" => self.overlay2,
            "subtext0" => self.subtext0,
            "subtext1" => self.subtext1,
            "text" => self.text,
            "red" => self.red,
            "orange" => self.orange,
            "yellow" => self.yellow,
            "lime" => self.lime,
            "green" => self.green,
            "aqua" => self.aqua,
            "skye" => self.skye,
            "snow" => self.snow,
            "blue" | "accent" => self.blue,
            "purple" => self.purple,
            "pink" => self.pink,
            _ => return None,
        })
    }

    fn accent(&self, hue: f32, sat: f32) -> u32 {
        // muted tan reads as yellow, saturated orange stays orange
        let hue = if (25.0..32.0).contains(&hue) && sat < 0.7 { 34.0 } else { hue };
        match hue {
            h if !(10.0..345.0).contains(&h) => self.red,
            h if h < 32.0 => self.orange,
            h if h < 70.0 => self.yellow,
            h if h < 95.0 => self.lime,
            h if h < 150.0 => self.green,
            h if h < 172.0 => self.aqua,
            h if h < 195.0 => self.skye,
            h if h < 205.0 => self.snow,
            h if h < 255.0 => self.blue,
            h if h < 292.0 => self.purple,
            _ => self.pink,
        }
    }

    /// The theme's colour for a grey of brightness `v` (0..255 on the xterm ramp).
    fn grey(&self, v: f32) -> Rgb {
        let stops: [(f32, u32); 11] = [(0.0, self.mantle), (28.0, self.surface0), (48.0, self.surface1), (68.0, self.surface2), (88.0, self.overlay0), (128.0, self.overlay1), (148.0, self.overlay2), (188.0, self.subtext0), (208.0, self.subtext1), (238.0, self.text), (255.0, self.text)];
        for w in stops.windows(2) {
            if v <= w[1].0 {
                let t = (v - w[0].0) / (w[1].0 - w[0].0);
                return mix(rgb_of(w[0].1), rgb_of(w[1].1), t);
            }
        }
        rgb_of(self.text)
    }

    fn map(&self, i: u8, bg: bool) -> Rgb {
        if i == 0 {
            return rgb_of(if bg { self.base } else { self.text });
        }
        let c = xterm_rgb(i);
        let (h, s, l) = hsl(c);
        if s < 0.12 {
            return self.grey((c.0 as f32 + c.1 as f32 + c.2 as f32) / 3.0);
        }
        let accent = rgb_of(self.accent(h, s));
        // dark shades are tints (diff backgrounds, selections), light ones the accent itself
        let mut f = (0.2 + (l - 0.19) / 0.36 * 0.8).clamp(0.2, 1.0);
        if luminance(rgb_of(self.base)) > 0.4 {
            // on a light theme a faint tint barely shows; make diffs and selections readable
            f = if f < 0.97 { (f * 1.5 + 0.12).min(0.85) } else { f };
        }
        if f > 0.97 {
            accent
        } else {
            mix(rgb_of(self.base), accent, f)
        }
    }
}

struct Tables {
    fg: [Rgb; 256],
    bg: [Rgb; 256],
}

fn tables() -> &'static Vec<Option<Tables>> {
    static T: OnceLock<Vec<Option<Tables>>> = OnceLock::new();
    T.get_or_init(|| {
        THEMES
            .iter()
            .map(|t| {
                t.pal.as_ref().map(|p| {
                    let mut tb = Tables { fg: [(0, 0, 0); 256], bg: [(0, 0, 0); 256] };
                    for i in 0..=255u8 {
                        tb.fg[i as usize] = p.map(i, false);
                        tb.bg[i as usize] = p.map(i, true);
                    }
                    tb
                })
            })
            .collect()
    })
}

static CUR: AtomicUsize = AtomicUsize::new(0);

pub fn current() -> usize {
    CUR.load(Ordering::Relaxed).min(THEMES.len() - 1)
}

pub fn current_def() -> &'static ThemeDef {
    &THEMES[current()]
}

pub fn is_classic() -> bool {
    THEMES[current()].pal.is_none()
}

pub fn find(id: &str) -> Option<usize> {
    let id = id.trim().to_lowercase().replace([' ', '_'], "-");
    THEMES.iter().position(|t| t.id == id).or_else(|| THEMES.iter().position(|t| t.id.ends_with(&format!("-{}", id))))
}

pub fn set(i: usize) {
    CUR.store(i.min(THEMES.len() - 1), Ordering::Relaxed);
}

/// What to draw for a palette-index pair: `None` is the terminal's default.
pub fn resolve(fg: u8, bg: u8) -> (Option<Rgb>, Option<Rgb>) {
    let (f, b) = match &tables()[current()] {
        Some(t) => (Some(t.fg[fg as usize]), Some(t.bg[bg as usize])),
        None => ((fg != 0).then(|| xterm_rgb(fg)), (bg != 0).then(|| xterm_rgb(bg))),
    };
    match (f, b) {
        (Some(f), Some(b)) => (Some(legible(f, b)), Some(b)),
        other => other,
    }
}

// ---- per-app choice, saved in ~/.config/mtui/themes ----

static APP: Mutex<String> = Mutex::new(String::new());
static WATCH: Mutex<Option<(Instant, Option<SystemTime>)>> = Mutex::new(None);

pub fn config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))).unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("mtui").join("themes")
}

/// All `key=theme` lines (apps, external terminals, and `*`).
pub fn read_all() -> Vec<(String, String)> {
    let s = std::fs::read_to_string(config_path()).unwrap_or_default();
    s.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).filter(|(k, _)| !k.is_empty() && !k.starts_with('#')).collect()
}

/// The saved theme for `key`, falling back to the `*` default.
pub fn lookup(key: &str) -> Option<usize> {
    let all = read_all();
    all.iter().find(|(k, _)| k == key).or_else(|| all.iter().find(|(k, _)| k == "*")).and_then(|(_, v)| find(v))
}

/// Record `key`'s theme.
pub fn save(key: &str, i: usize) -> std::io::Result<()> {
    let path = config_path();
    let mut all = read_all();
    match all.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = THEMES[i].id.to_string(),
        None => all.push((key.to_string(), THEMES[i].id.to_string())),
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text: String = std::iter::once("# mtui themes: app=theme (see `mtheme`); * is the default\n".to_string()).chain(all.iter().map(|(k, v)| format!("{}={}\n", k, v))).collect();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

fn stamp() -> Option<SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}

/// Name this program and pick its theme: `$MTUI_THEME`, else its saved one.
pub fn init(app: &str) {
    *APP.lock().unwrap() = app.to_string();
    let env = std::env::var("MTUI_THEME").ok().and_then(|v| find(&v));
    set(env.or_else(|| lookup(app)).unwrap_or(0));
    *WATCH.lock().unwrap() = Some((Instant::now(), stamp()));
}

/// Move to the next (or previous) theme and remember it. Returns its name.
pub fn cycle(dir: isize) -> &'static str {
    let n = THEMES.len() as isize;
    let i = (current() as isize + dir).rem_euclid(n) as usize;
    set(i);
    let app = APP.lock().unwrap().clone();
    if !app.is_empty() {
        let _ = save(&app, i);
        *WATCH.lock().unwrap() = Some((Instant::now(), stamp()));
    }
    THEMES[i].name
}

/// Has the theme file chosen a different theme for this app? Cheap enough to
/// call from every tick; checks the file at most once a second. When it
/// returns true the screen needs a full redraw.
pub fn poll() -> bool {
    let app = APP.lock().unwrap().clone();
    if app.is_empty() || std::env::var_os("MTUI_THEME").is_some() {
        return false;
    }
    let mut w = WATCH.lock().unwrap();
    let Some((last, seen)) = w.as_mut() else { return false };
    if last.elapsed().as_millis() < 1000 {
        return false;
    }
    *last = Instant::now();
    let now = stamp();
    if now == *seen {
        return false;
    }
    *seen = now;
    drop(w);
    let want = lookup(&app).unwrap_or(0);
    if want != current() {
        set(want);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // the active theme is global, so tests that change it take turns
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn every_theme_keeps_the_ui_colours_readable() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (ti, t) in THEMES.iter().enumerate() {
            set(ti);
            // text on bars / selections / accent tags, as the apps draw them
            for (fg, bg) in [(252, 236), (255, 237), (246, 236), (16, 110), (16, 114), (16, 180), (16, 176), (75, 24), (203, 236), (250, 234), (0, 237), (0, 0), (246, 0)] {
                let (f, b) = resolve(fg, bg);
                let (f, b) = (f.unwrap_or((255, 255, 255)), b.unwrap_or((0, 0, 0)));
                assert!(contrast(f, b) >= 4.4, "{}: fg {} on bg {} is {:.1}", t.id, fg, bg, contrast(f, b));
            }
        }
        set(0);
    }

    #[test]
    fn mapping_keeps_the_roles() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set(find("evergarden-winter").unwrap());
        let (_, base) = resolve(0, 0);
        assert_eq!(base, Some(rgb_of(0x1E2528)));
        let (text, _) = resolve(255, 0);
        assert_eq!(text, Some(rgb_of(0xF8F9E8)));
        // the "tan" accent index becomes the theme's yellow; reds stay red
        assert_eq!(resolve(0, 180).1, Some(rgb_of(0xF5D098)));
        assert_eq!(resolve(0, 203).1, Some(rgb_of(0xF57F82)));
        // a dark green diff background is a tint, darker than the accent
        let add = resolve(0, 22).1.unwrap();
        assert!(luminance(add) < luminance(rgb_of(0xCBE3B3)) && add != rgb_of(0x1E2528));
        set(0);
    }

    #[test]
    fn light_theme_colours_keep_their_hue_and_read() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set(find("summer").unwrap());
        let (_, base) = resolve(0, 0);
        let base = base.unwrap();
        for (idx, hue) in [(203u8, 0.0f32), (114, 100.0), (75, 215.0), (176, 300.0), (180, 40.0)] {
            let (fg, _) = resolve(idx, 0);
            let fg = fg.unwrap();
            assert!(contrast(fg, base) >= 4.5, "index {} reads {:.1}", idx, contrast(fg, base));
            let (h, s, _) = hsl(fg);
            assert!(s > 0.15, "index {} went grey (s={:.2})", idx, s);
            let d = (h - hue).abs().min(360.0 - (h - hue).abs());
            assert!(d < 70.0, "index {} hue {} vs {}", idx, h, hue);
        }
        // a diff-add background is visibly green-tinted, not near-white
        let add = resolve(0, 22).1.unwrap();
        assert!(contrast(add, base) > 1.12 && add.1 > add.0);
        set(0);
    }

    #[test]
    fn names_resolve() {
        assert_eq!(find("Evergarden Winter"), Some(1));
        assert_eq!(find("summer"), Some(4));
        assert_eq!(find("nope"), None);
    }
}
