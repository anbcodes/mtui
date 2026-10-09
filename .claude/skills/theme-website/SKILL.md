---
name: theme-website
description: Theme a website to match the user's mtheme theme (Evergarden etc.) with custom CSS applied through Firefox, one site at a time. Use when the user wants a site (Fastmail, GitHub, Jira, Slack, any web app) to follow their terminal/desktop theme, asks to "theme <site>", fix colours on a site that is already themed, or add a site to mtheme.
---

# Theming a website with mtheme

mtheme can hand Firefox per-site CSS. A site is a **template**: plain CSS with
`{{placeholders}}` for the active theme's colours, and a header comment naming its
domains. `mtheme set firefox <theme>` renders every template, wraps each in
`@-moz-document domain(...)`, and writes it into each profile's `userContent.css`
chain, so only those sites change. Classic renders nothing. Firefox reads the files
at startup, so a restart shows the result; the console one-liner below previews
instantly.

Only ever write CSS. Never ask for credentials, cookies or tokens, and don't paste
page content into templates.

## Commands

```
mtheme sites                         list templates (built-in and ~/.config/mtui/sites/*.css)
mtheme sites new NAME DOMAIN         create ~/.config/mtui/sites/NAME.css from a skeleton
mtheme sites render NAME [THEME]     print the CSS for a theme (default: Firefox's current)
mtheme sites inject NAME [THEME]     print a JS one-liner that applies the CSS to the open page
mtheme sites probe                   print a console script that reports how a site themes itself
mtheme set firefox THEME             regenerate Firefox's CSS (restart Firefox to see it)
mtheme setup firefox                 once: let Firefox read mtheme's files (user.js pref + @imports)
```

Placeholders: palette colours (`base mantle crust surface0..2 overlay0..2 subtext0..1
text red orange yellow lime green aqua skye snow blue purple pink`, or a `#hex`) as
`{{blue}}`; `{{rgb c}}` → `r, g, b`; `{{alpha c 0.5}}`; `{{mix a b 0.2}}` (0 = a, 1 = b);
`{{lighten c 0.1}}`, `{{darken c 0.1}}`; `{{ink c}}` = text that reads on `c`;
`{{scheme}}` = `dark`|`light`; `{{name}}` = theme name.

Handy tricks: hover/active shades that work on dark *and* light themes are
`{{mix blue text 0.15}}` (moves toward the text colour); tinted status backgrounds
are `{{mix base red 0.14}}` with borders at ~0.45; always give foregrounds on
accent-coloured fills as `{{ink accent}}`; a "white-on-colour" avatar needs
`{{darken c 0.45}}`.

## Workflow

1. **Prereqs.** `mtheme list` should show a `firefox` row; if the user never ran
   `mtheme setup firefox`, offer to (it edits their `user.js`, `userChrome.css`,
   `userContent.css` — say so first).
2. **Find how the site themes itself**, in this order (stop at the first that works):
   - **CSS custom properties** (best): the root element has a class or attribute
     like `t-dark` / `data-theme` / `data-color-mode` and hundreds of `--color-*`
     variables. Override those on the same selector.
   - **A handful of semantic classes**: override those with `background`, `color`,
     `border-color`.
   - **Hard-coded colours**: last resort; target containers only and keep it small.
     Do not try to recolour images.
3. **Inspect without logging in.** With the built-in browser open the site's public
   page (login/landing) and read what you can with JavaScript: `document.documentElement.className`,
   rules in `document.styleSheets` whose selector is `:root…`/`html…` and which declare `--*`
   properties (print names with their light and dark values), and the computed
   colours of the key elements. The login page often ships the *whole* stylesheet,
   so the variable list is complete even though the signed-in screens are hidden.
4. **Inspect the signed-in app via the user** (you cannot log in): ask them to open
   the site in Firefox on the screen that matters, press F12, paste the output of
   `mtheme sites probe`, and paste back the clipboard text. It reports root
   classes/attributes, custom-property groups, the biggest background/text colours
   with sample selectors, and the colours of anchors (header, nav, buttons…). It
   contains class names and colours, not message text; tell them to skim it before
   pasting.
5. **Write the template** (`mtheme sites new NAME domain.com`, then edit). Map by
   meaning, not by value: page background → `base`; sidebar/header/backdrop →
   `mantle`; hover/selected rows → `surface0/1` or a blue tint; borders →
   `surface1/2`; primary text → `text`; secondary → `subtext0`; disabled/hint →
   `overlay1`; links, primary buttons, focus rings → `blue`; danger → `red`;
   success → `green`; warnings → `yellow`; unread/new markers → an accent.
   - Declarations must be `!important`: user stylesheets lose to the site's own
     rules otherwise.
   - Put variables on the selector the site uses, and include both light and dark
     variants (`:root.t-light, :root.t-dark`) so the user's theme always wins.
   - Add `color-scheme: {{scheme}} !important;` so scrollbars and form controls match.
6. **Preview without restarting.** `mtheme sites inject NAME THEME` prints a one-liner;
   the user pastes it into the console on the live page (or you run it in the built-in
   browser on a public page — Chromium ignores `@-moz-document`, which is why the
   one-liner carries only the inner CSS). Check **both a dark and the light theme**
   (`evergarden-summer`) — contrast problems show up there first. Iterate with
   the user's screenshots or descriptions ("the sidebar is still white").
7. **Ship.** `mtheme set firefox <their theme>` and ask them to restart Firefox.
   If it is generally useful, move the template to `crates/mtheme/sites/NAME.css`
   and add it to `BUILTIN` in `crates/mtheme/src/sites.rs` (the unit test renders
   every built-in for every theme).

## Pitfalls

- A rule that "does nothing": the site sets the variable later with higher
  specificity or inline; raise yours (`html:root.t-dark`), or target the elements.
- Text in images/SVG/canvas won't change; fix SVG icons with `fill`/`stroke`
  variables when the site exposes them.
- iframes get the rule too when their own domain matches; third-party embeds don't.
- Shadow DOM: custom properties inherit through it, selectors don't.
- Contrast: pastel accents are meant as fills; use `{{ink c}}` for text on them and
  `{{mix c text 0.3}}` for coloured text on the page background.
- `domain("example.com")` also matches `www.` and `app.` subdomains.

## Worked example: Fastmail

`crates/mtheme/sites/fastmail.css` (built in). Fastmail is the easy kind: the root
element carries `t-light` or `t-dark`, and one stylesheet defines ~220 custom
properties under `:root.t-light` / `:root.t-dark` — `--theme-color-header`,
`--theme-color-accent-{10,60,100,110,120}`, `--ui-page-color-{bg,fg,border}*`,
`--ui-{critical,warning,success,informative}-color-*`, `--ui-button-*`, `--ui-input-*`,
`--ui-layer-*`, avatar and quote colours. They were listed from the public
login page with a script that walked `document.styleSheets` for those two selectors.
The template overrides the ~170 that carry colour, using the same palette roles
for both modes. Not covered: the marketing/login backdrop gradient (hard-coded) and
purely decorative illustrations.
