# mdiff — watch a git tree change, and edit the diff

`mdiff` is mvi with a diff drawn in, beside a list of what changed. It shows the changes in a git working tree as they happen, made for watching another program (an AI session, a code generator) edit your tree and stepping in at any moment: the diff view *is* the file, open in mvi, so you edit it directly. Under the changes it can list a notes file and the code the notes point at.

```sh
cargo build --release -p mdiff
mdiff                                  # this branch: everything since it came off its parent
mdiff -b origin/main DIR               # since DIR's branch forked from origin/main
mdiff -n NOTES.md -x EXPLANATION.md    # also list notes and their code, and show a log
mdiff -p                               # print the changed files and exit
mdiff ~/wt/ICP-1234                    # a directory of checkouts: each against its own parent
```

- **The diff, inline.** A changed file opens in the editor with what the parent branch had shown above where it was (red rows, not part of the file), added and changed lines tinted green with `+` in the gutter, and unchanged stretches folded to one `⋯ 42 unchanged lines` row, keeping three lines of context next to each change. The diff is worked out from the editor's text against the parent's, so it follows as you type, before you save.
- **Expand and shrink.** Every fold opens with `za`/`zo`, Enter or a click, and closes again with `zc`; a change folds away too (`zc` on it), leaving one `⋯ 3 changed lines, 1 removed` row. `zR` opens everything, `zM` folds everything. `]c [c` jump between changes, and a closed fold is one line to `j`, `k` and scrolling. Folds stay as you left them while the file changes.
- **It's mvi.** All of mvi works in the editor: motions, operators, search, `:w`, undo. `Ctrl-W` moves between the editor and the list, `]f [f` go to the next or previous file in the list without leaving the editor, `:q` quits.
- **The whole branch.** The diff runs from where the branch forked from its parent, uncommitted and untracked files included: the parent is `-b REF`, else the branch it was created from (as git recorded it, following branches made from branches), else the remote's default branch. The fork point is recomputed as you go, so merging the parent in later doesn't bring its changes into view.
- **Several checkouts.** Given a directory that isn't a checkout, mdiff watches the checkouts in it and one level down (`System`, `Modules/Core`), not following symlinks, so in a ticket tree where unchanged repos are symlinks only the ticket's worktrees show. The list groups files by repo, with each repo's branch. Each repo's parent comes from `-b`, else `.claude/pair/BASE` (`REF` for all, or `DIR REF` lines), else the `base` that `wt` recorded in `.wt.json`, else what git recorded.
- **Live.** inotify on every directory of each checkout, and on its git directory for commits and branch switches, says which repo changed; git runs only for that one, once a burst of writes settles, so idle costs nothing and a change shows in about a fifth of a second. Without inotify (or out of watches) the status line says so and mdiff polls, at four times the length of a refresh but no more often than every 3 s. With follow on (`f` in the list, the default) the editor jumps to the first file that changed, unless you're mid-edit, have unsaved changes or just pressed a key.
- **Fresh lines.** A `▎` in the gutter marks the lines added by the change that last touched the file, so a long diff still shows what just happened. Files with fresh lines are bold in the list.
- **Someone else's edits.** Files open in the editor that you haven't modified follow changes made on disk, so a save never writes back a stale copy; if a file changes under unsaved edits, the status line says so (`:e!` takes the version on disk).
- **Notes.** With `-n FILE`, the list has a second group: the notes file, then every code location it mentions (`path:line`, `path:from-to`, or a bare path, as long as the file exists). The notes open in the editor too, with `›` on the lines that refer to code; Enter on one goes there. A location opens its file with those lines tinted blue (and its diff, if it changed); locations in changed files say so in the list.
- **Why.** With `-x FILE`, the newest entry of FILE is shown under the editor with its age (`x` in the list hides it). The log is newest first: entries are `## ` sections (`## src/app.rs:120 what changed`, then a line or two of why), and the first one is shown. An agent that adds an entry with each change gets its reasons shown beside its edits. The log itself is left out of the file list.

In a tree with `.claude/pair/` (what the `pair` launcher sets up), its `BASE`, `NOTES.md` and `EXPLANATION.md` are the defaults for `-b`, `-n` and `-x`, so `mdiff` alone shows a whole pair session.

## Keys

| In the editor | |
|---|---|
| everything mvi does | editing, motions, search, `:w`, `u` |
| `]c` `[c` | next / previous change |
| `za` `zo` `zc` | toggle / open / close the fold at the cursor (a change too) |
| `zR` `zM` | open / close every fold |
| Enter, click | open a closed fold; on a notes line that refers to code, go there |
| `]f` `[f` | next / previous item in the list |
| `Ctrl-W` | to the list |
| `:q` | quit |

| In the list | |
|---|---|
| `j` `k`, `g` `G` | pick a file, the notes or a location; it opens in the editor |
| Enter, Tab, `Ctrl-W` | to the editor |
| `f` | follow changes on / off |
| `x` | show / hide the explanation entry |
| `s` | show / hide the list |
| `?` | keys |
| `q` | quit (not with unsaved changes) |

The mouse picks from the list, and in the editor works as in mvi; a click on a fold opens it.
