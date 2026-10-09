# mjira

A minimal Jira client for the terminal: your issues, a project board, and everything you do to an issue on a normal day (move it, assign it, comment, log work) without opening the browser. It talks to Jira's REST API, so what you change is the real thing, and the screen shows what Jira says.

```sh
# Jira Cloud
export JIRA_URL=https://you.atlassian.net
export JIRA_EMAIL=you@example.com
export JIRA_API_TOKEN=…           # id.atlassian.com > Security > API tokens
mjira                             # or `mjira ABC` to start on a project

# Jira Server / Data Center: a personal access token, no email
export JIRA_URL=https://jira.example.com
export JIRA_API_TOKEN=…
```

All of it can instead be lines in `~/.config/mjira/config`: `url`, `email`, `token`, `project`, `api cloud|server`, `mouse off`, `images off`. Cloud is assumed for `*.atlassian.net`, Server otherwise; `api` (or `$JIRA_API`) overrides it. Cloud uses API v3 (documents in Atlassian Document Format, account ids, token-paged search); Server and Data Center use v2 (wiki text, user names, offset paging). An email with a Server URL sends Basic auth with the token as the password.

## Tabs

`1` issues assigned to you that aren't done, `2` ones you watch, `3` recently viewed, `4` a project's open issues, `5` the project **board**, `6` search results. The project is the one you give, or else the one most of your issues are in; `C-k` picks another (type a key if it isn't in the list). `s` searches: real JQL (`assignee = currentUser() AND sprint in openSprints()`) is used as written, plain words become a text search, and an issue key like `ABC-123` opens that issue. `f` runs one of your favourite filters, `/` filters the list you're looking at.

Lists page as you scroll, refresh every minute, and show type, key, summary, priority, status (coloured by Jira's category), assignee and age.

## The board

One column per status of the project, in workflow order (to do, in progress, done), with the issues that are open or were finished in the last two weeks. `h`/`l` move between columns, `j`/`k` between cards, `Enter` opens one. `<` and `>` move the card to the previous or next column: mjira looks up the transition into that status and applies it (and tells you if the workflow doesn't allow it from there). The card moves at once and the next refresh confirms it. The mouse works too: click a card (double-click opens), wheel scrolls a column.

## Issues

`Enter` opens an issue: summary, status, type, priority, assignee, reporter, dates, labels, components, sprint and story points where the site has them, then the description, attachments, subtasks, links and a timeline of comments mixed with status, assignee and priority changes. While you read, the list you came from is the sidebar (the current issue marked): `J`/`K` (or `L`/`H`) go to the next or previous issue without going back, `Tab` focuses the sidebar so `j`/`k` do the same, `b` hides it. The open issue refreshes every 30 seconds.

| | |
|---|---|
| `t` | transition: pick from what the workflow allows from the current status |
| `a` `i` | assign (pick from assignable users, or unassigned), assign to me |
| `p` | priority |
| `c` | comment |
| `e` `#` | edit the summary, set the labels |
| `c` `E` `C` | comment, edit the description, edit your latest comment. These open mvi docked at the bottom (your `~/.config/mvi/config` applies): `:wq` sends, `:q!` cancels, `gwip` reflows a paragraph, `:set tw=N` sets the width. Existing text opens as markdown-style markup (`**bold**`, lists, ``` fences, `> quotes`, tables, `:::info` panels) and converts back losslessly: anything it cannot show as text stays in ` ```adf-json ` / ` ```wiki-raw ` blocks, so saving never drops content (Server/Data Center use wiki markup, Cloud uses ADF) |
| `w` `W` | watch / stop watching, log work (`1h 30m`, then an optional note) |
| `n` | new issue in the project (pick a type, type the summary) |
| `o` `y` | open in the browser, copy the URL |

`t` `a` `i` `p` `o` `y` also work on the selected issue in any list or on the board. What you write is sent as Jira text: blank lines separate paragraphs, `- ` and `1. ` lines make lists, ``` fences make code blocks, and `**bold**`, `` `code` ``, `[text](url)` and bare links become formatting. Enter sends, Alt-Enter adds a line, Esc cancels.

Descriptions and comments are shown as markdown. Image attachments are shown inline in terminals with the kitty graphics protocol (kitty, ghostty, WezTerm; `MTUI_IMAGES=1` forces it, `images off` disables it), fetched with your credentials; click one to see it fullscreen. Changes to a status or assignee show on screen immediately and are replaced by the server's answer on the next refresh, including an error if Jira refused (a transition that needs a required field says so).

Press `?` for every key.
