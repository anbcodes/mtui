# mvi — a tiny, batteries-included vim-like editor

`mvi` is a modal editor written in Rust. It depends only on `libc` and the shared [`mtui`](../mtui) crate. It aims for:

* **A small footprint:** a ~0.9 MB binary that uses ~3 MB of RSS when idle.
* **Low latency:** ~1 ms per keystroke on a 100k-line file.
* **Low bandwidth:** it keeps a double-buffered cell grid and sends only the cells that changed, with a single `write` per frame. Keys that arrive together (fast typing, pastes, ssh bursts) are processed before the next redraw.
* **Batteries included:** syntax highlighting, compiler diagnostics, spelling and grammar checks for prose, completion, a fuzzy file finder, project grep, goto-definition, and an OSC 52 clipboard.

```sh
cargo build --release -p mvi
./target/release/mvi [+LINE] file...        # `mvi -` reads stdin
```

## Markdown preview

`:preview` renders the buffer in a pager inside the terminal. `:mmd` opens a live HTML preview in the browser (rendered by [mmd](../mmd)): it shows what you type, follows the cursor line, and double-clicking a block in the browser jumps to its source. `:mmd stop` ends it.

## Spelling and grammar

Markdown, `.txt`, git commit messages and mail drafts (`.eml`: the Subject and the body, not quoted `>` lines) are checked as you type. Spelling mistakes get a red curly underline and grammar problems a blue one; the status line counts them (`S3 G1`), and the message line explains the one under the cursor.

| | |
|---|---|
| `]s` `[s` | next / previous problem |
| `z=` | suggestions for a misspelt word (or any word under the cursor), or the fix for a grammar problem; pick one to apply it (`u` undoes) |
| `zg` / `zG` | accept the word: in `~/.config/mvi/words` (one word per line) / for this session |
| `:spell` | every problem in the buffer, in a picker |
| `:spellgood WORD` | add a word to `~/.config/mvi/words` |
| `:set spell` / `nospell` | check this buffer (any file type) / stop; `set nospell` in `~/.config/mvi/config` turns the automatic checking off |
| `:set spelllang=en_GB` | another dictionary (default: from `$LANG`, else `en_US`) |
| `:set nogrammar` | spelling only |
| `:set spellstrict` | English: accept only the dictionary's spelling (by default both *colour* and *color*, *organise* and *organize* pass) |

Spelling uses the system's Hunspell dictionaries (`/usr/share/hunspell`, `~/.local/share/hunspell`, `$DICPATH`, or `~/.config/mvi/spell/LANG.{aff,dic}`; Debian/Ubuntu: `hunspell-en-us`) and falls back to `/usr/share/dict/words`. Words are looked up the way Hunspell does it (a stem plus prefix/suffix rules), straight from the `.dic` text, so en_US takes about 2 MB and loads in ~10 ms, on first use. Code is skipped: fenced and indented code blocks, `inline code`, URLs, e-mail addresses, HTML tags, link targets, front matter, math, shell prompts, and words that look like identifiers (`snake_case`, `camelCase`, `ALLCAPS`, anything with digits or dots). A short list of developer words (`config`, `repo`, `backend`, `regex`, …) is accepted too.

Grammar rules are few and chosen to be right when they fire: repeated words (*the the*), *a*/*an* (by sound: *an hour*, *a union*, *a European*; acronyms are left alone), a lower-case sentence start, lower-case *i*, space before punctuation, missing space after a comma, doubled punctuation, and well-known confusions (*could of*, *more then*, *its a*, *your welcome*, *their is*, *per say*, *free reign*, …).

Results are cached per line: after the first pass (a few ms for a typical document; ~0.2 s for 40k lines) a keystroke rechecks just the changed line. Suggestions combine weighted edit distance (swapped letters, doubled letters, neighbouring keys and vowel confusions are cheap), the dictionary's REP table, split words (*alot* → *a lot*) and an n-gram scan of the dictionary for the harder cases; ~10 ms each.

## As a library

`mvi` is also a crate: `mvi::Editor` is the editor, `mvi::run(&mut ed, &mut input)` is the event loop (the caller owns raw mode and the `mtui::term::Input`), `mvi::edit_file(&mut input, path, line)` runs a full-screen session on a file, and `mvi::Pane` embeds the editor in a rectangle of the host's `mtui::screen::Screen`: the host passes it keys (`pane.key`), draws it (`pane.draw(&mut screen, (x, y), (w, h))`, which returns the cursor) and checks `pane.finished()`. mmail uses a pane to write messages.

## Features

| | |
|---|---|
| Editing | Normal/insert/visual/visual-line modes, operators `d c y > < gc gu gU g~` with motions, counts and text objects (`iw aw i( a{ i" ip …`), registers (`"a`–`"z`, append with `"A`, `"0`, `"_`, `"+`), `.` repeat, macros (`q`/`@`), marks, undo/redo, `Ctrl-A`/`Ctrl-X`, auto-indent, bracket auto-dedent |
| Search | Built-in regex engine (PCRE-style subset with captures and smartcase), incremental search, highlighting, `* # n N`, `:s` with `\1` and `&`, `:g`/`:v` |
| Syntax | Table-driven highlighter with per-line state caching. Supports Rust, C, C++, Python, JS/TS, Go, shell, JSON, TOML, YAML, Markdown, Lua, Java/Kotlin/C#/Swift, Ruby, Zig, Erlang, HTML/XML, CSS and SQL, plus Makefile and Dockerfile |
| Spelling | Hunspell dictionaries, markdown-aware, with high-precision grammar rules. `]s [s` jump, `z=` suggests or fixes, `zg` adds a word (see below) |
| Diagnostics | Runs the language's checker in a background thread on save (or with `:check` / `<space>c`). Shows gutter signs, underlines and inline messages. `]d [d ]e [e` jump through them, `K` shows the full text, `<space>d` lists them, `]q [q` work across files |
| Completion | Pops up as you type. Sources are buffer words (ranked by distance from the cursor), symbols from all open buffers (with the definition line as a signature hint), language keywords, and file paths. Use `Tab`/`Ctrl-N`/`Ctrl-P` to select and `Enter` to accept |
| Navigation | `<space>f` fuzzy file finder (respects simple `.gitignore` entries), `<space>b` buffers, `<space>s` symbols in the current file, `<space>/` or `:grep` for a project regex search, `gd` goto definition (current buffer → open buffers → project), `gf` to open the file under the cursor |
| Mouse | Click to move the cursor (also in insert mode), drag to select in visual mode, double-click selects a word, triple-click selects a line, the wheel scrolls, and the back/forward side buttons walk the jump history (`G`, `gd`, `%`, search, file and picker jumps) in normal mode, as do `Ctrl-O` (back) and `Tab` / `Ctrl-I` (forward). Shift+drag uses the terminal's own selection, and `:set nomouse` turns the mouse off |
| Misc | Bracketed paste, `<space>y` copies to the system clipboard via OSC 52 (works over ssh), `:[range]!cmd` filters text through a command, `:r !cmd`, `:sort`, `:norm`, `:m`/`:t`, `Ctrl-Z` suspends, files are saved atomically |

`:help` (or `mvi --help-keys`) prints the full key reference.

## Diagnostics

Each language has an ordered list of checker commands. `mvi` uses the first one whose program is on `$PATH`:

| Language | Checkers |
|---|---|
| rust | `cargo check --message-format=short` (run where `Cargo.toml` is), otherwise `rustc --error-format=short …` |
| c / cpp | `cc -fsyntax-only -Wall -Wextra`, `c++ -fsyntax-only` |
| python | `ruff check --output-format=concise`, otherwise a syntax check with Python's `ast` module |
| js / ts | `tsc --noEmit` (if there is a `tsconfig.json`), otherwise `node --check` |
| go | `go vet ./...` (in the `go.mod` root), otherwise `gofmt -e` |
| others | sh: `shellcheck -f gcc` or `bash -n` · json: `json.tool` or `jq` · lua: `luac -p` · ruby: `ruby -wc` · zig: `zig ast-check` · erlang: `erlc -Wall` · java: `javac` |

The output parser understands these formats:

* gcc/clang/rustc-short: `file:line:col: severity: msg`
* rustc's long `--> file:line:col` blocks
* Python tracebacks
* tsc's `file(line,col)`
* bash's `file: line N:`
* a generic `line N column M`

So most other tools work as is. To override or add a checker, in `~/.config/mvi/config` or at the `:` prompt:

```vim
checker python mypy --no-error-summary {file}
checker rust Cargo.toml|cargo clippy --message-format=short
set noautocheck          " don't check on save; use :check manually
```

`{file}` is the absolute path. `Marker|cmd` runs `cmd` in the nearest ancestor directory that contains `Marker`.

## Config

`~/.config/mvi/config` holds one ex command per line. For example:

```vim
set rnu
set ts=8
set list
```

Indentation (tabs vs. spaces and the width) is detected per file.

## Design notes

* **Buffer**
  * Stored as a `Vec<String>` of lines.
  * Undo history is a log of insert/delete edits, grouped per command or insert session, rather than snapshots.
* **Highlighting**
  * The highlighter caches the lexer state at the start of each line and invalidates it from the first edited line onward.
  * Only visible lines are coloured.
* **Rendering**
  * The renderer draws into a cell grid. `Screen::flush` diffs it against the previous frame and emits cursor moves plus SGR codes only for changed cells, using 256-color codes.
  * Autowrap is disabled, so the last column is safe to draw.
* **Background work**
  * Checkers run in a background thread, so the editor never blocks on a compiler.
  * The main loop uses `poll(2)` with no timeout unless a check is running.
* **Panics**
  * Builds use `panic = "abort"`.
  * A panic hook restores the terminal before the process exits.

## Limitations

* No soft wrap (long lines scroll horizontally), no splits, and no block-visual mode.
* No LSP: completion and goto-definition are heuristic.
* The regex engine is backtracking, with PCRE syntax rather than vim's "magic" syntax.
