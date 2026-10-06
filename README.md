# m* — tiny terminal tools

A small family of fast, low-bandwidth terminal programs written in Rust. They share a support crate (`mtui`, libc only). `mvi` needs nothing else; `mslack` and `mgh` also use `mhttp`, a small HTTP client over rustls, and `mimg` for inline images.

| Tool | What it is | Size |
|---|---|---|
| [`mvi`](crates/mvi) | A vim-like modal editor with syntax highlighting, diagnostics, completion, a fuzzy finder and grep | ~0.9 MB |
| [`mslack`](crates/mslack) | A Slack client: live updates (Socket Mode or RTM), channels, DMs, threads, reactions, edits, unread tracking | ~2 MB |
| [`mgh`](crates/mgh) | A GitHub client: review requests, your PRs and issues, the notification inbox and any repo; a code review view with syntax-highlighted diffs and inline comments; approve, request changes, close, merge | ~1.5 MB |
| [`mtui`](crates/mtui) | The shared terminal library (not a program) | |
| [`mhttp`](crates/mhttp) | The shared HTTP/1.1 client (not a program) | |
| [`mimg`](crates/mimg) | Image decoding and the image cache for the kitty graphics protocol (not a program) | |

```sh
cargo build --release              # builds everything into target/release/
cargo build --release -p mslack    # or just one tool
cargo test
```

## What `mtui` provides

| Module | |
|---|---|
| `term` | Raw mode, key decoding (CSI/SS3, UTF-8, Alt, bracketed paste, opt-in SGR mouse), resize flag, Ctrl-Z suspend, a panic hook that restores the terminal, and a `Waker` self-pipe so background threads can interrupt a blocking key read |
| `kitty` | Kitty graphics protocol (also ghostty and WezTerm): transmit once, place with cropping so images scroll |
| `syntax` | Table-driven syntax highlighting, shared by mvi and mgh's code view |
| `screen` | A double-buffered cell grid. `flush` diffs it against the previous frame and sends only the changed cells in one `write`. Also handles OSC 52 clipboard |
| `picker` | A generic fuzzy picker (`Picker<T: Label>`) with its keys and bottom-docked drawing. mvi uses it for files, buffers, symbols and grep results; mslack uses it for channels and links, and mgh for repos. It also handles wheel and click |
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
crates/mgh/         GitHub client
crates/mhttp/       shared HTTP client
crates/mimg/        shared image support
```

To add a tool, create `crates/<name>` with `mtui.workspace = true` in its dependencies.
