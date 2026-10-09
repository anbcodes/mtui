# mtheme

Pick a colour theme for each app on its own, and for the terminal itself.

```
mtheme                       the picker, previewing the highlighted row's theme
mtheme list                  themes, and what each app / terminal uses now
mtheme set mvi evergarden-fall
mtheme set kitty summer      (a unique suffix is enough)
mtheme set all winter
mtheme setup kitty           add the include line kitty needs (also: ghostty)
```

Themes: `classic` (the original colours on your terminal's own background) and
the four Evergarden seasons — `evergarden-winter`, `-fall`, `-spring` (dark) and
`-summer` (light). Palettes are from <https://evergarden.moe>.

**Apps** (mvi, mmail, mjira, mslack, mgh) remember their theme in
`~/.config/mtui/themes` (`app=theme` lines; `*=theme` is a default). They check
that file every second, so changing a theme here changes running apps, too.
Inside an app `Ctrl-T` steps to the next theme (mvi: `:theme`, `:theme NAME`).
`MTUI_THEME=NAME` overrides it for one run.

**Terminals**: kitty (`~/.config/kitty/current-theme.conf`, reloaded with
`kitty @ set-colors` or `SIGUSR1`), ghostty (`themes/mtheme`, `theme = mtheme`,
`SIGUSR2`) and alacritty (`mtheme.toml`, which you `import`). `mtheme setup`
appends the include line for kitty and ghostty; alacritty is shown as a hint.
Only terminals found on this machine are listed.

**Firefox**: for each install's profile mtheme writes `chrome/mtheme-chrome.css`
(toolbar, tabs, URL bar, panels, sidebar) and `chrome/mtheme-content.css` (the
new tab page and other `about:` pages, never websites). `mtheme setup firefox`
(or `I`) adds an `@import` line at the top of `userChrome.css` / `userContent.css`
and `toolkit.legacyUserProfileCustomizations.stylesheets` to `user.js`, leaving
the rest of your files alone. Firefox reads these only at startup, so restart it.

**Waybar** (the bar under sway): `~/.config/mtui/waybar.css` recolours your bar
(`mtheme setup waybar` adds one `@import` at the end of the stylesheet waybar
loads) and the desktop background follows the bar's colour: `swaymsg output * bg`
now, and `~/.config/mtui/sway-bg` is re-applied by a line `setup` adds to a
`bar.sh` next to your stylesheet. Classic uses whatever `window#waybar` says in
your own CSS.

**Rofi** gets `~/.config/rofi/mtheme.rasi`, imported from `config.rasi`.

**Websites** (Firefox): `mtheme sites` lists per-site CSS templates — CSS with
`{{blue}}`, `{{mix base blue 0.2}}`, `{{ink blue}}` … placeholders and a
`/* mtheme-site: domain.com */` header — which `mtheme set firefox` renders for the
active theme into `userContent.css` inside `@-moz-document domain(...)`. Fastmail is
built in; yours go in `~/.config/mtui/sites/NAME.css` (`mtheme sites new NAME DOMAIN`).
`mtheme sites inject NAME` prints a console one-liner to try a template live,
`mtheme sites probe` a console script that reports how a signed-in site colours
itself. The `theme-website` skill (`.claude/skills/`) walks through theming a new site.

**Sites you can edit the stylesheet of** (an unpacked Electron app, a userstyle):
`mtheme sites css NAME [THEME]` prints just the CSS, between `/* >>> mtheme … */` and
`/* <<< mtheme … */` markers so an older chunk can be stripped before appending a new one.
The built-in `claude-app` template re-colours Claude's UI that way: it replaces the nine
`--cds-*` colour ramps (everything else, including the light/dark swap, derives from them) and
the legacy `--bg-*`/`--text-*`/`--accent-*` tokens. It is not applied through Firefox.

**Claude desktop** is not driven by mtheme: the app refuses to start when a debugging switch such as
`--remote-debugging-port` is present, so CSS cannot be injected into it. Editing its renderer
stylesheet by hand with the chunk above works, until an update replaces the file.

Keys in the picker: `j k` row, `h l` previous / next theme, `1`–`5` pick,
`a` give every target the highlighted row's theme, `I` set up a terminal, `q`.
