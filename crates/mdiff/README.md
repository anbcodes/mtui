# mdiff — watch a git tree change, and edit what you see

`mdiff` shows the changes in a git working tree as they happen: the changed files down the side, the selected file's diff with syntax highlighting, and the lines the latest change added marked in the gutter. It is made for watching another program (an AI session, a code generator) edit your tree, and for stepping in: Enter opens the file at the cursor in an mvi pane under the diff, and the diff keeps updating while you edit.

```sh
cargo build --release -p mdiff
mdiff                                  # changes in this repo since HEAD
mdiff -b "$(git rev-parse HEAD)" DIR   # since now: later commits stay in the diff
mdiff -x .claude/pair/EXPLANATION.md   # also show the newest entry of a log
mdiff -p                               # print the changed files and exit
```

- **Live.** A background thread re-reads `git diff BASE` and the untracked files twice a second and redraws only when something changed. With follow on (`f`, the default) the view jumps to the first file that changed and puts the change a third of the way down.
- **Fresh lines.** A `▎` in the gutter marks the lines added by the change that last touched the file, so a long diff still shows what just happened. Files with fresh lines are bold in the list.
- **Edit in place.** Enter (or `e`) opens the file at the line under the cursor in an embedded mvi, with your mvi config; a deleted line opens where it was. `Ctrl-W` from normal mode switches between the diff and the editor, `:w` saves and the diff above updates, `:q` closes the pane. Enter on another file opens it as another buffer. Buffers you haven't modified follow changes made on disk, so a save never writes back a stale copy; if the file changes under unsaved edits, the status line says so (`:e!` takes the version on disk).
- **Why.** With `-x FILE`, the newest entry of FILE is shown under the diff with its age (`x` hides it). The log is newest first: entries are `## ` sections (`## src/app.rs:120 what changed`, then a line or two of why), and the first one is shown. An agent that adds an entry with each change gets its reasons shown beside its edits. The log itself is left out of the file list.

## Keys

| Key | |
|---|---|
| `j` `k` `↓` `↑`, `Ctrl-D` `Ctrl-U`, `g` `G` | move |
| `J` `K`, `Tab` `Shift-Tab` | next / previous file |
| `n` `N` | next / previous hunk |
| `←` `→` | scroll sideways |
| `Enter` `e` | edit the file at this line |
| `Ctrl-W` | switch between the diff and the editor pane |
| `f` | follow changes on / off |
| `x` | show / hide the explanation entry |
| `s` | show / hide the file list |
| `?` | keys |
| `q` | quit |

The mouse scrolls the list and the diff, a click picks a file or a line, and a click in the editor gives it the keys.
