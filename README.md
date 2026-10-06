# m* — tiny terminal tools

A small family of fast, low-bandwidth terminal programs written in Rust. They share one support crate. `mtui` and `mvi` depend only on `libc`, and `mslack` adds rustls for TLS.

| Tool | What it is | Size |
|---|---|---|
| [`mvi`](crates/mvi) | A vim-like modal editor with syntax highlighting, diagnostics, completion, a fuzzy finder and grep | ~0.9 MB |
| [`mslack`](crates/mslack) | A Slack client: live updates (Socket Mode or RTM), channels, DMs, threads, reactions, edits, unread tracking | ~2 MB |
| [`mtui`](crates/mtui) | The shared library (not a program) | |

```sh
cargo build --release              # builds everything into target/release/
cargo build --release -p mslack    # or just one tool
cargo test
```

## What `mtui` provides

| Module | |
|---|---|
| `term` | Raw mode, key decoding (CSI/SS3, UTF-8, Alt, bracketed paste, opt-in SGR mouse), resize flag, Ctrl-Z suspend, a panic hook that restores the terminal, and a `Waker` self-pipe so background threads can interrupt a blocking key read |
| `screen` | A double-buffered cell grid. `flush` diffs it against the previous frame and sends only the changed cells in one `write`. Also handles OSC 52 clipboard |
| `picker` | A generic fuzzy picker (`Picker<T: Label>`) with its keys and bottom-docked drawing. mvi uses it for files, buffers, symbols and grep results; mslack uses it for channels and links. It also handles wheel and click |
| `lineedit` | A text input with readline keys, history, multi-line text and wrapped drawing |
| `fuzzy` | Subsequence scoring and filtering |
| `regex` | A backtracking PCRE-style regex engine |
| `json` | A JSON parser and string quoting |
| `wrap` | Word wrapping by display width |
| `base64` | Base64 encoding (OSC 52, WebSocket handshake) |

All the tools look and behave alike: the same picker, the same status-line style, and `Ctrl-Z` to suspend.

## Layout

```
Cargo.toml          workspace (shared release profile: LTO, panic=abort, stripped)
crates/mtui/        shared library
crates/mvi/         editor
crates/mslack/      Slack client
```

To add a tool, create `crates/<name>` with `mtui.workspace = true` in its dependencies.
