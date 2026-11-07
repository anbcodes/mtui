# mdiff — watch a git tree change, and edit what you see

`mdiff` shows the changes in a git working tree as they happen: the changed files down the side, the selected file's diff with syntax highlighting, and the lines the latest change added marked in the gutter. It is made for watching another program (an AI session, a code generator) edit your tree, and for stepping in: Enter opens the file at the cursor in an mvi pane under the diff, and the diff keeps updating while you edit. Under the changes it can list a notes file and the code the notes point at.

```sh
cargo build --release -p mdiff
mdiff                                  # this branch: everything since it came off its parent
mdiff -b origin/main DIR               # since DIR's branch forked from origin/main
mdiff -n NOTES.md -x EXPLANATION.md    # also list notes and their code, and show a log
mdiff -p                               # print the changed files and exit
```

- **The whole branch.** The diff runs from where the branch forked from its parent, uncommitted and untracked files included: the parent is `-b REF`, else the branch it was created from (as git recorded it), else the remote's default branch. The fork point is recomputed as you go, so merging the parent in later doesn't bring its changes into view.
- **Live.** A background thread re-reads the diff and the untracked files twice a second and redraws only when something changed. With follow on (`f`, the default) the view jumps to the first file that changed and puts the change a third of the way down.
- **Fresh lines.** A `▎` in the gutter marks the lines added by the change that last touched the file, so a long diff still shows what just happened. Files with fresh lines are bold in the list.
- **Edit in place.** Enter (or `e`) opens the file at the line under the cursor in an embedded mvi, with your mvi config; a deleted line opens where it was. `Ctrl-W` from normal mode switches between the diff and the editor, `:w` saves and the diff above updates, `:q` closes the pane. Enter on another file opens it as another buffer. Buffers you haven't modified follow changes made on disk, so a save never writes back a stale copy; if the file changes under unsaved edits, the status line says so (`:e!` takes the version on disk).
- **Notes.** With `-n FILE`, the list has a second group: the notes file, then every code location it mentions (`path:line`, `path:from-to`, or a bare path, as long as the file exists). The notes view marks the lines that refer to code with `›`, and Enter on one goes to it. A location opens the file at those lines, highlighted, with the notes line it came from pinned above and this session's added lines shown as added; locations in changed files say so. Both follow the files as they change, and a change elsewhere is announced on the status line instead of pulling you away from what you're reading.
- **Why.** With `-x FILE`, the newest entry of FILE is shown under the diff with its age (`x` hides it). The log is newest first: entries are `## ` sections (`## src/app.rs:120 what changed`, then a line or two of why), and the first one is shown. An agent that adds an entry with each change gets its reasons shown beside its edits. The log itself is left out of the file list.

In a tree with `.claude/pair/` (what the `pair` launcher sets up), its `BASE`, `NOTES.md` and `EXPLANATION.md` are the defaults for `-b`, `-n` and `-x`, so `mdiff` alone shows a whole pair session.

## Keys

| Key | |
|---|---|
| `j` `k` `↓` `↑`, `Ctrl-D` `Ctrl-U`, `g` `G` | move |
| `J` `K`, `Tab` `Shift-Tab` | next / previous item in the list: files, notes, code locations |
| `n` `N` | next / previous hunk; in the notes, line that refers to code; in a location's file, block of added lines |
| `←` `→` | scroll sideways |
| `Enter` | edit the file at this line; in the notes, go to the code the line refers to |
| `e` | edit here (in the notes, the notes) |
| `Ctrl-W` | switch between the diff and the editor pane |
| `f` | follow changes on / off |
| `x` | show / hide the explanation entry |
| `s` | show / hide the list |
| `?` | keys |
| `q` | quit |

The mouse scrolls the list and the diff, a click picks a file or a line, and a click in the editor gives it the keys.
