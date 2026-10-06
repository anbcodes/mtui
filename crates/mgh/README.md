# mgh

A minimal GitHub client for the terminal. Lists are fetched with `If-None-Match`, so a refresh of an unchanged list costs one tiny 304.

```sh
export GITHUB_TOKEN=ghp_…     # or `token ghp_…` in ~/.config/mgh/config, or the token gh saved
mgh                           # the repo tab follows the current directory's origin remote
mgh owner/repo                # start on a given repo
```

The token needs `repo` and `notifications` access (fine-grained: pull requests, issues, contents and metadata read, plus write to comment, review and merge).

## Tabs

`1` PRs waiting for your review, `2` your open PRs, `3` issues assigned to you, `4` the notification inbox, `5` a repo's open PRs and issues (`C-k` picks another; type `owner/repo` to go to one that isn't listed).

## Reviewing code

Open a pull request and press `d`. Changed files show as syntax-highlighted diffs with line numbers and the existing inline comment threads. `]`/`[` (or Tab) move between files, `f` picks one, `m` marks it viewed, `}`/`{` jump hunks, `n`/`N` jump comments, `e` shows the whole file at the PR head instead of just the hunks.

Move to a line and press `c` to comment on it (`v` first selects several lines). Comments queue in a pending review, shown inline and counted in the status bar; `x` drops one. `S` submits them with a summary, or `a` / `X` approve or request changes with them. `r` replies to a thread right away. Pending comments survive leaving the item, but not quitting mgh.

Images in descriptions, comments and added or changed `.png`/`.jpg` files are shown inline in kitty, ghostty and WezTerm (set `MTUI_IMAGES=1` to force it, `0` or `images off` in the config to disable). Images on private repos that need a browser session can't be fetched and show as a link.

Press `?` for every key. Lists: `j k`, `Enter`, `/` filter, `r` refresh, `o` browser, `y` copy URL, `m` mark read. An item shows the description, labels, reviews, CI checks, mergeability and the comment timeline; `c` comments, `a` approves, `X` requests changes, `x` closes or reopens, `M` merges.

## Config

`~/.config/mgh/config`:

```
token ghp_…
mouse off
images off
```

`GITHUB_API_URL` points mgh at GitHub Enterprise or a test server.
