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

Keys in the picker: `j k` row, `h l` previous / next theme, `1`–`5` pick,
`a` give every target the highlighted row's theme, `I` set up a terminal, `q`.
