# mslack — a tiny Slack client

`mslack` is a keyboard-driven Slack client in the style of `mvi`. It is a ~2 MB binary that uses ~5 MB of RSS, and it redraws only the cells that change. New messages, edits and reactions are pushed over a WebSocket, using Socket Mode or RTM depending on your token. If neither is available, it polls.

```sh
cargo build --release -p mslack
SLACK_TOKEN=xoxp-… SLACK_APP_TOKEN=xapp-… ./target/release/mslack [channel]
```

Inline images are clickable: a click shows the image fullscreen, and any key or click closes it.

## Getting a token

1. Go to <https://api.slack.com/apps>, choose **Create New App → From an app manifest**, and paste [`slack-app-manifest.yml`](slack-app-manifest.yml).
2. Install the app to your workspace. Some workspaces need an admin to approve it.
3. Copy the **User OAuth Token** (`xoxp-…`) from **OAuth & Permissions**.
4. For live updates (optional but recommended): under **Basic Information → App-Level Tokens**, generate a token with the `connections:write` scope (`xapp-…`).

Then either export them as `SLACK_TOKEN` and `SLACK_APP_TOKEN`, or put them in `~/.config/mslack/config`:

```
token xoxp-…
apptoken xapp-…
```

Keep that file private (`chmod 600`). The status line shows `● live` when the WebSocket is connected and `○ polling` while it is down.

### Or: use your browser login (no app)

The Slack web client uses a session token (`xoxc-…`) together with a `d` cookie (`xoxd-…`). mslack can use the same pair:

```
token xoxc-…
cookie xoxd-…
```

While logged in at `app.slack.com`, get the cookie from DevTools → Application → Cookies → `d`. Get the token by running this in the DevTools console:

```js
Object.values(JSON.parse(localStorage.localConfig_v2).teams).map(t => [t.name, t.token])
```

Live updates then use RTM, so no app token is needed. This is unofficial: Slack doesn't document it, some workspaces forbid it, and the pair works only as long as that browser session does. Treat it like a password.

### Which live connection gets used

| Credentials | Live updates |
|---|---|
| `token xoxp-…` + `apptoken xapp-…` | Socket Mode |
| `token xoxc-…` + `cookie xoxd-…` | RTM |
| `token xoxp-…` from an old "classic" app | RTM |
| `token xoxp-…` alone (from the manifest app) | none; polls every 5 s |

## Keys

The interface is modal, like vim. You start in normal mode and browse; `i` opens the composer.

| Normal mode | |
|---|---|
| `j` `k` / arrows | select message (`k` past the top loads older history) |
| `Ctrl-D` `Ctrl-U` | half page |
| `G` / `gg` | latest / oldest |
| `Enter` `l` `t` | open the selected message's thread |
| `r` | reply in thread |
| `h` `q` `Esc` | leave thread |
| `i` `a` | compose |
| `e` / `D` | edit / delete your own message |
| `+` | add or remove a reaction (Tab completes the emoji name) |
| `y` | copy message text (OSC 52, works over ssh) |
| `v` | select a range: `j`/`k`/`G`/`Ctrl-D` extend it, `y` copies it as a transcript (`[10:42] alice: …`, with date lines), `Esc` cancels |
| `gx` | open a link from the message |
| `Tab` `J` / `Shift-Tab` `K` | next / previous conversation |
| `u` | next unread conversation |
| `Ctrl-K` | fuzzy conversation picker (also works while composing) |
| `b` | toggle sidebar |
| `R` | refresh |
| `?` | help |
| `q` | quit |

| Compose | |
|---|---|
| `Enter` / `Alt-Enter` | send / newline |
| `Tab` | complete `@user`, `#channel`, `:emoji:` (press again to cycle) |
| `Up` on an empty line | edit your last message |
| `Ctrl-P` `Ctrl-N` | select messages without leaving the composer |
| `Ctrl-A` `Ctrl-E` `Ctrl-W` `Ctrl-U` `Ctrl-K` `Alt-B` `Alt-F` | readline editing |
| `Esc` | back to normal mode (the draft is kept) |

| Mouse | |
|---|---|
| click a conversation | open it |
| wheel over messages | scroll freely (the selection stays put; scrolling to the bottom follows new messages again, and scrolling past the top loads older history) |
| wheel over the sidebar | scroll the conversation list |
| click a message | select it; click again (double-click) to open its thread |
| drag across messages | select a range (dragging past the edge scrolls); then `y` copies it |
| click `↳ N replies` | open the thread |
| click a reaction | add or remove yours |
| click a link | open it in the browser |
| click the composer | start typing, with the cursor where you clicked |
| click the header | leave a thread |
| click in a picker | choose that item (clicking above it cancels) |

While mouse capture is on, select text with Shift+drag (Option+drag in some macOS terminals). To turn the mouse off, add `mouse off` to the config file.

Outgoing `@name`, `#channel` and `@here` are turned into real Slack mentions. Incoming mentions, links, `code`, `*bold*`, `_italic_` and `> quotes` are highlighted, and common `:shortcodes:` are shown as emoji.

## How it stays light

* **Push.** A background thread holds one WebSocket. With an app token it uses [Socket Mode](https://api.slack.com/apis/socket-mode) and acknowledges each event envelope. Otherwise it tries [RTM](https://api.slack.com/rtm) with your user token. RTM also reports reads made on other devices, which clears unread markers here. Message, edit, delete, reaction and channel-membership events go to the UI. After 30 s of silence it pings; after two silent periods it treats the link as dead. On a drop it reconnects with backoff, and the open conversation is re-fetched to fill any gap. If the token can't use either method, the thread stops and mslack polls. While connected, the only polling is a refresh every 5 minutes as a safety net.
* **Polling fallback.** Without live events, or while the socket is down, the open conversation (and thread) is polled every 5 s with `oldest=<newest ts>`, which usually returns an empty list. Every 30 s the latest 30 messages are re-fetched to pick up edits, reactions and deletions.
* **Unread tracking.** Pushed events mark conversations unread as they arrive. `client.counts` gives the starting unread and mention counts in one call, and resyncs them (for example after you read something on your phone). If the token can't use it (it isn't a public API method) and there's no socket, mslack falls back to checking one other conversation's newest message every 3 s, round-robin. A new DM or mention rings the terminal bell.
* **Connections.** The HTTP/1.1 client is built in, with TLS from rustls and the system's CA certificates. API calls run on four worker threads, and each keeps one connection alive. Each request has a key, so the same request is never in flight twice. Rate-limited calls wait out `Retry-After`. A self-pipe wakes the UI when results arrive. When nothing is happening, the process sleeps in `poll(2)`.

## Why not MCP?

Slack's MCP server is made for AI agents. Its tools return prose summaries rather than structured messages, and it needs OAuth through an app approved for MCP. A UI needs timestamps, user IDs, thread metadata and reactions, so the Web API is a better fit and needs no extra process.

## Limitations

* No file upload or download. Files are shown by name.
* No search, no channel joining or creation, no user status or presence.
* Block Kit messages are shown through their plain `text` or attachment fallback.
* The emoji table covers only common shortcodes. Custom workspace emoji are shown as `:name:`.

## Images

PNG and JPEG attachments are shown inline in kitty, ghostty and WezTerm (not through tmux). `MTUI_IMAGES=1` forces it on, `MTUI_IMAGES=0` or `images off` in the config turns it off. Other terminals, and other file types, show as `📎 name`.
