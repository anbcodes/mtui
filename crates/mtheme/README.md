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

**Claude desktop is not supported.** It deliberately refuses to start when a
debugging switch such as `--remote-debugging-port` is present, which is the only
non-invasive way to inject CSS into an Electron app; patching its files would
defeat that protection and break on updates.

Keys in the picker: `j k` row, `h l` previous / next theme, `1`–`5` pick,
`a` give every target the highlighted row's theme, `I` set up a terminal, `q`.
