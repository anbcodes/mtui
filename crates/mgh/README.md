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

Press `?` for every key. Lists: `j k`, `Enter`, `/` filter, `r` refresh, `o` browser, `y` copy URL, `m` mark read. An item shows the description, labels, reviews, CI checks, mergeability and the comment timeline; `d` shows the colored diff (`]`/`[` jump between files), `c` comments, `a` approves, `X` requests changes, `x` closes or reopens, `M` merges.

## Config

`~/.config/mgh/config`:

```
token ghp_…
mouse off
```

`GITHUB_API_URL` points mgh at GitHub Enterprise or a test server.
