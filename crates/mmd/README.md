# mmd

A minimal markdown renderer. It makes clean, **printable** HTML and previews it in the browser with **live reload**; [mvi](../mvi) drives it for scroll-synced editing (`:mmd`).

```sh
mmd notes.md                  # open a live preview; the page updates as the file is saved
mmd -o notes.html notes.md    # a standalone HTML file (add --embed to inline local images)
mmd - < notes.md > notes.html # stdin to stdout
```

## What it renders

Headings (ATX and setext, with anchors and an outline on wide screens), emphasis, strikethrough, code spans, links (inline, reference, auto and bare URLs), images, block quotes, GitHub alerts (`> [!NOTE]`), nested ordered, unordered and task lists, tables with alignment, fenced and indented code with syntax highlighting (the same highlighter mvi uses), rules, hard breaks and the HTML people embed in READMEs. Scripts, iframes, forms, event-handler attributes and `javascript:` URLs are dropped. Every block carries `data-line`, its source line.

The page follows your colour scheme on screen. For print (Ctrl-P in the browser, or "save as PDF") it switches to a serif face on white with sensible margins, hides the outline and page chrome, keeps headings with their text, avoids splitting code blocks, tables, quotes and images across pages, wraps long code lines, and writes external link addresses out after the link text.

## Live preview

`mmd FILE` serves the page on `127.0.0.1` (random port, or `-p`), opens it, and watches the file. Changes arrive over server-sent events and are swapped in without reloading, so your scroll position stays. The directory of the file is served too, so relative images work. The pill at the bottom right shows the connection and toggles scroll sync.

## With mvi

In mvi, `:mmd` opens the live preview of the current buffer. It shows what you **type**, not just what you save, scrolls the page to the line your cursor is on, and double-clicking a block in the browser moves the cursor to its source line. `:mmd stop` ends it. (`:preview` is still the in-terminal pager.)

## As a library

`mmd::render(src, &Options)` returns the body HTML, headings and title; `mmd::to_html` a complete page; `mmd::Server` is the preview: `set_source`, `scroll_to(line)`, `take_goto()`, `watch(path)`. Any editor can drive it.
