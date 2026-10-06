# mmail

A minimal Fastmail client for the terminal. It speaks JMAP, the protocol Fastmail's own web and mobile apps use, so it isn't a copy of your mail: it is the same mailbox. Folders, read and starred state, drafts, sent mail and labels are the server's, and a change made anywhere shows up everywhere within a moment.

```sh
export FASTMAIL_TOKEN=fmu1-…     # or `token fmu1-…` in ~/.config/mmail/config
mmail
```

Make the token in Fastmail under Settings → Privacy & Security → Integrations → New API token. It needs **Email** access, plus **Email submission** to send. (With a read-only token mmail works as a reader; writing says why it can't.)

## How it stays in sync

- **Push.** mmail keeps an EventSource connection open (`● live` in the status line). When the server's mail state changes, whether from the web client, your phone, a filter rule or delivery, mmail resyncs the folder list and counts, the message list and the open conversation. If the connection drops it shows `○ polling`, resyncs every 45 seconds, and reconnects on its own (resyncing as soon as it does).
- **Writes go to the server.** Archiving, trashing, moving, labelling, starring and marking read are `Email/set` changes. They apply on screen at once, and the next resync replaces whatever was guessed with what the server says (including a failure, which is reported).
- **Same model as the web client.** Lists are conversations (collapsed threads); labels are folders a message can be in several of; opening a conversation marks it read; sending saves the message through a draft and moves it from Drafts to Sent; replying sets `$answered` and forwarding `$forwarded` on the original, so the web client shows the reply arrow.
- Nothing is cached on disk: start it anywhere, on any machine, and you see the current mailbox.

## Keys

Press `?` for them all.

| | |
|---|---|
| `j k` `Enter` `gg G` | move, open, first / last |
| `Tab` `h` / `C-k` | folder pane / jump to any folder (fuzzy) |
| `/` | search (below); `Esc` leaves it |
| `c` `r` `R` `f` | compose, reply, reply all, forward |
| `e` `#` `!` | archive, trash (in Trash: delete for good, with a prompt), spam |
| `u` `s` | toggle unread, toggle star |
| `m` `M` | move to a folder, add or remove a label |
| `x` | mark conversations; the actions above then apply to all of them |
| `o` `w` | in a conversation: open a link, save an attachment |
| `n p` `Enter` `a` | in a conversation: next / previous message, expand or collapse, expand all |
| `J K` (also `L H`) | next / previous folder in the list; next / previous conversation while reading |
| `E` | edit a draft (`Enter` on a draft in Drafts does the same) |

Search takes plain words (matched anywhere in the message) and narrowing terms: `from:` `to:` `cc:` `subject:` `in:folder` `is:unread|read|starred|draft` `has:attachment` `before:2026-01-31` `after:2026-01-01`. It covers every folder except Trash and Spam unless you say `in:`.

## Reading, images and the mouse

While a conversation is open, the folder's conversations become the sidebar (the current one is marked): `J`/`K` move through them without going back to the list, and `Tab` focuses the sidebar so `j`/`k` do the same. Attached PNG and JPEG images show inline in terminals with the kitty graphics protocol (kitty, ghostty, WezTerm; `MTUI_IMAGES=1` forces it, `images off` in the config disables it). Images that an HTML message would fetch from other servers are *not* loaded (that is how senders learn you opened a message); `I` loads them for that message.

The mouse works everywhere: click a folder or a row (double-click opens), click an image to see it fullscreen (any key or click closes it), click a message header to expand or collapse it, an attachment to save it, the links line to pick a link; the wheel scrolls whatever is under it. In the list the first columns are buttons: `✓` marks, `●` toggles read, `★` stars; right-click marks a row.

## Writing

mmail opens your `$VISUAL` / `$EDITOR` (or `vi`) on the message as text:

```
From: Me <me@example.com>
To: friend@example.org
Cc:
Bcc:
Subject: Hello
Attach: ~/photo.jpg

Body starts after the first blank line…
```

Add recipients (comma separated), more `Attach:` lines for files, or change `From:` to any of your identities, including wildcard aliases (replies pick the address the message was sent to). `Keep:` lines list attachments already on the server (a forwarded message's, or a draft's); delete one to drop it. Your identity's signature is filled in. When you leave the editor: `y` sends, `d` saves a draft (it appears in the web client's Drafts), `e` edits again, `q` discards. If sending fails the text is kept.

## Config

`~/.config/mmail/config`:

```
token fmu1-…
mouse off
images off
session https://api.fastmail.com/jmap/session
```

`session` (or `$MMAIL_SESSION_URL`) points mmail at another JMAP server. Attachments are saved to `$MMAIL_DOWNLOADS` (default `~/Downloads`).

## What it doesn't do

HTML composing (messages are sent as plain text), contacts and calendars, rules and settings, inline images, and offline use. Those belong to the web client, and everything you do there is picked up here.
