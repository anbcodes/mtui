// Per-site CSS themes. A site is a template: ordinary CSS with `{{...}}`
// placeholders for the active theme's colours, plus one header comment naming
// the domains it applies to:
//
//     /* mtheme-site: fastmail.com, app.fastmail.com */
//     :root { --bg: {{base}}; --hover: {{mix base blue 0.15}}; }
//
// Rendering wraps the result in `@-moz-document domain(...)` and the Firefox
// target appends it to the user stylesheet, so only those sites are touched.
//
// Placeholders (names are palette colours — base mantle crust surface0..2
// overlay0..2 subtext0..1 text red orange yellow lime green aqua skye snow
// blue purple pink — or a #hex):
//     {{blue}}                 #rrggbb
//     {{rgb blue}}             r, g, b
//     {{alpha base 0.6}}       rgba(r, g, b, 0.6)
//     {{mix base blue 0.2}}    20% of the way from base to blue
//     {{lighten blue 0.1}}     {{darken blue 0.1}}
//     {{ink blue}}             text colour that reads on blue
//     {{hsl blue}}             "h s% l%", for sites that wrap it in hsl(var(--x) / a)
//     {{ramp 0.3}}             the palette's neutrals as one scale, lightest (0) to darkest (1)
//     {{chroma blue 0.3}}      the same lightness as {{ramp 0.3}}, in blue's hue
//     {{raw TEXT}}             TEXT unchanged (handy inside {{if-dark A | B}})
//     {{scheme}}               dark | light

use mtui::theme::{self, hex, mix, rgb_of, Pal, Rgb, ThemeDef};
use std::path::PathBuf;

pub struct Site {
    pub name: String,
    pub domains: Vec<String>,
    pub body: String,
    pub builtin: bool,
    /// Applied through Firefox (`mtheme set firefox`) unless the template says `mtheme-firefox: no`.
    pub firefox: bool,
}

const BUILTIN: [(&str, &str); 2] = [("fastmail", include_str!("../sites/fastmail.css")), ("claude-app", include_str!("../sites/claude-app.css"))];

fn user_dir() -> PathBuf {
    theme::config_path().with_file_name("sites")
}

pub fn parse(name: &str, text: &str, builtin: bool) -> Option<Site> {
    let head = text.lines().find(|l| l.contains("mtheme-site:"))?;
    let doms = head.split("mtheme-site:").nth(1)?.trim().trim_end_matches("*/").trim();
    let domains: Vec<String> = doms.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
    (!domains.is_empty()).then(|| Site { name: name.to_string(), domains, body: text.to_string(), builtin, firefox: !text.contains("mtheme-firefox: no") })
}

/// Built-in templates, overridden or extended by `~/.config/mtui/sites/*.css`.
pub fn all() -> Vec<Site> {
    let mut v: Vec<Site> = BUILTIN.iter().filter_map(|(n, t)| parse(n, t, true)).collect();
    if let Ok(rd) = std::fs::read_dir(user_dir()) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "css")).collect();
        files.sort();
        for f in files {
            let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(site) = std::fs::read_to_string(&f).ok().and_then(|t| parse(&name, &t, false)) {
                v.retain(|s| s.name != name);
                v.push(site);
            }
        }
    }
    v
}

fn color(p: &Pal, word: &str) -> Result<Rgb, String> {
    if let Some(h) = word.strip_prefix('#') {
        return u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6).map(rgb_of).ok_or_else(|| format!("bad colour {}", word));
    }
    p.get(word).map(rgb_of).ok_or_else(|| format!("unknown colour '{}'", word))
}

/// The twelve neutrals ordered lightest first, whatever the theme's polarity.
fn neutrals(p: &Pal) -> Vec<Rgb> {
    let mut v: Vec<Rgb> = ["base", "mantle", "crust", "surface0", "surface1", "surface2", "overlay0", "overlay1", "overlay2", "subtext0", "subtext1", "text"].iter().filter_map(|n| p.get(n)).map(rgb_of).collect();
    v.sort_by(|a, b| theme::luminance(*b).partial_cmp(&theme::luminance(*a)).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Luminance for position `u` (0 = the theme's lightest neutral, 1 = its darkest), spaced
/// evenly in contrast terms, so a step that had 4.5:1 against the page in the original
/// design keeps a proportional contrast in the theme.
fn lum_at(p: &Pal, u: f32) -> f32 {
    let n = neutrals(p);
    let (hi, lo) = (theme::luminance(n[0]) + 0.05, theme::luminance(*n.last().unwrap_or(&n[0])) + 0.05);
    (hi.ln() - u.clamp(0.0, 1.0) * (hi.ln() - lo.ln())).exp() - 0.05
}

/// The colour on the path `a` → `b` with luminance `target` (luminance is monotonic along it).
fn along(a: Rgb, b: Rgb, target: f32) -> Rgb {
    let (la, lb) = (theme::luminance(a), theme::luminance(b));
    if (target - la).abs() < 1e-5 || (la - lb).abs() < 1e-6 {
        return a;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if (theme::luminance(mix(a, b, mid)) < target) == (la < lb) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    mix(a, b, (lo + hi) / 2.0)
}

/// The theme's neutrals as one scale: the colour at position `u` along them.
fn ramp(p: &Pal, u: f32) -> Rgb {
    let n = neutrals(p);
    let target = lum_at(p, u);
    for w in n.windows(2) {
        if theme::luminance(w[1]) <= target {
            return along(w[0], w[1], target);
        }
    }
    *n.last().unwrap_or(&n[0])
}

/// A scale of colour `c` at position `u`: the same lightness as `ramp` at `u`, in c's hue.
fn chroma(p: &Pal, c: Rgb, u: f32) -> Rgb {
    let n = neutrals(p);
    let target = lum_at(p, u);
    let (light, dark) = (mix(n[0], c, 0.10), mix(*n.last().unwrap_or(&n[0]), c, 0.22));
    if target >= theme::luminance(c) {
        along(c, light, target.min(theme::luminance(light)))
    } else {
        along(c, dark, target.max(theme::luminance(dark)))
    }
}

fn num(w: Option<&&str>) -> Result<f32, String> {
    w.and_then(|s| s.parse().ok()).ok_or_else(|| "expected a number".to_string())
}

fn expr(def: &ThemeDef, p: &Pal, e: &str) -> Result<String, String> {
    let e = e.trim();
    // {{if-dark A | B}}: A on a dark theme, B on a light one
    if let Some(rest) = e.strip_prefix("if-dark") {
        let (a, b) = rest.split_once('|').ok_or("if-dark needs 'A | B'")?;
        let light = theme::luminance(rgb_of(p.base)) > 0.4;
        return expr(def, p, if light { b } else { a });
    }
    let w: Vec<&str> = e.split_whitespace().collect();
    let light = theme::luminance(rgb_of(p.base)) > 0.4;
    Ok(match w.as_slice() {
        ["scheme"] => (if light { "light" } else { "dark" }).into(),
        ["name"] => def.name.into(),
        [c] => hex(color(p, c)?),
        // text passed through unchanged, for if-dark branches that are not colours: {{raw var(--x)}}
        ["raw", rest @ ..] => rest.join(" "),
        ["hsl", rest @ ..] if !rest.is_empty() => {
            let inner = expr(def, p, &rest.join(" "))?;
            let (h, sat, l) = theme::hsl(color(p, &inner)?);
            format!("{:.0} {:.1}% {:.1}%", h, sat * 100.0, l * 100.0)
        }
        ["ramp", t] => hex(ramp(p, num(Some(t))?)),
        ["chroma", c, t] => hex(chroma(p, color(p, c)?, num(Some(t))?)),
        ["rgb", c] => {
            let (r, g, b) = color(p, c)?;
            format!("{}, {}, {}", r, g, b)
        }
        ["alpha", c, a] => {
            let (r, g, b) = color(p, c)?;
            format!("rgba({}, {}, {}, {})", r, g, b, num(Some(a))?)
        }
        ["mix", a, b, t] => hex(mix(color(p, a)?, color(p, b)?, num(Some(t))?)),
        ["lighten", c, t] => hex(mix(color(p, c)?, (255, 255, 255), num(Some(t))?)),
        ["darken", c, t] => hex(mix(color(p, c)?, (0, 0, 0), num(Some(t))?)),
        ["ink", c] => hex(theme::legible(if light { rgb_of(p.text) } else { rgb_of(p.base) }, color(p, c)?)),
        _ => return Err(format!("don't understand {{{{{}}}}}", e)),
    })
}

/// The template's CSS with placeholders filled in for `def`.
pub fn render_body(site: &Site, def: &ThemeDef) -> Result<String, String> {
    let Some(p) = &def.pal else { return Ok(String::new()) };
    let mut out = String::new();
    let mut rest = site.body.as_str();
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find("}}") else { return Err(format!("{}: unclosed {{{{", site.name)) };
        out.push_str(&expr(def, p, &rest[i + 2..i + j]).map_err(|e| format!("{}: {}", site.name, e))?);
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A site as a rule for Firefox's userContent.css.
pub fn render(site: &Site, def: &ThemeDef) -> Result<String, String> {
    let body = render_body(site, def)?;
    if body.is_empty() {
        return Ok(String::new());
    }
    let doms: Vec<String> = site.domains.iter().map(|d| format!("domain(\"{}\")", d)).collect();
    Ok(format!("/* site: {} */\n@-moz-document {} {{\n{}\n}}\n", site.name, doms.join(", "), body.trim()))
}

/// Every site, for the Firefox stylesheet; problems are reported and the site skipped.
pub fn render_all(def: &ThemeDef) -> (String, Vec<String>) {
    let (mut css, mut errs) = (String::new(), Vec::new());
    for s in all().into_iter().filter(|s| s.firefox) {
        match render(&s, def) {
            Ok(c) => css.push_str(&c),
            Err(e) => errs.push(e),
        }
    }
    (css, errs)
}

/// Console script that reports how a site themes itself (see `mtheme sites probe`).
pub const PROBE_JS: &str = include_str!("../sites/probe.js");

pub const SKELETON: &str = r#"/* mtheme-site: example.com */
/* Palette: base mantle crust surface0..2 overlay0..2 subtext0..1 text
   red orange yellow lime green aqua skye snow blue purple pink.
   {{name}} {{rgb c}} {{alpha c 0.5}} {{mix a b 0.2}} {{lighten c 0.1}} {{darken c 0.1}} {{ink c}} {{scheme}} */
:root {
  color-scheme: {{scheme}} !important;
  /* --page-bg: {{base}} !important; */
}
"#;

/// Create `~/.config/mtui/sites/NAME.css` from the skeleton.
pub fn create(name: &str, domain: &str) -> Result<PathBuf, String> {
    let path = user_dir().join(format!("{}.css", name));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    std::fs::create_dir_all(user_dir()).map_err(|e| e.to_string())?;
    std::fs::write(&path, SKELETON.replace("example.com", domain)).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_render_and_errors_are_reported() {
        let site = parse("t", "/* mtheme-site: a.com, b.org */\nx{a:{{base}};b:{{rgb text}};c:{{alpha base 0.5}};d:{{mix base text 0}};e:{{scheme}}}", false).unwrap();
        let w = &theme::THEMES[1];
        assert_eq!(render_body(&site, w).unwrap().lines().nth(1).unwrap(), "x{a:#1e2528;b:248, 249, 232;c:rgba(30, 37, 40, 0.5);d:#1e2528;e:dark}");
        let r = render(&site, w).unwrap();
        assert!(r.contains("@-moz-document domain(\"a.com\"), domain(\"b.org\") {"));
        assert_eq!(render(&site, &theme::THEMES[0]).unwrap(), "");
        let bad = parse("t", "/* mtheme-site: a.com */ {{nope}}", false).unwrap();
        assert!(render_body(&bad, w).unwrap_err().contains("unknown colour"));
        assert!(parse("t", "no header", false).is_none());
    }

    #[test]
    fn ramps_run_light_to_dark_in_every_theme() {
        for t in theme::THEMES.iter().filter(|t| t.pal.is_some()) {
            let p = t.pal.as_ref().unwrap();
            let l = |x: &str| theme::luminance(rgb_of(u32::from_str_radix(&expr(t, p, x).unwrap()[1..], 16).unwrap()));
            assert!(l("ramp 0") > l("ramp 0.5") && l("ramp 0.5") > l("ramp 1"), "{}", t.id);
            assert!(l("chroma red 0") > l("chroma red 0.5") && l("chroma red 0.5") > l("chroma red 1"), "{}", t.id);
            // chroma keeps the lightness of the neutral scale
            let (a, b) = (l("ramp 0.4"), l("chroma red 0.4"));
            assert!((a - b).abs() < 0.03, "{}: ramp {} chroma {}", t.id, a, b);
        }
        let w = &theme::THEMES[1];
        assert_eq!(expr(w, w.pal.as_ref().unwrap(), "hsl red").unwrap(), "358 85.5% 72.9%");
        assert_eq!(expr(w, w.pal.as_ref().unwrap(), "if-dark raw var(--a) | raw #fff").unwrap(), "var(--a)");
        let l = &theme::THEMES[4];
        assert_eq!(expr(l, l.pal.as_ref().unwrap(), "if-dark raw var(--a) | raw #fff").unwrap(), "#fff");
    }

    #[test]
    fn builtin_templates_render_for_every_theme() {
        for t in theme::THEMES.iter() {
            for s in all().iter().filter(|s| s.builtin) {
                assert!(render(s, t).is_ok(), "{} / {}", s.name, t.id);
            }
        }
    }
}
