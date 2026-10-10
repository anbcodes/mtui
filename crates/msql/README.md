# msql — write, save and run SQL against your database

`msql` is a terminal SQL client built around [`mvi`](../mvi): you write queries in the real editor (an embedded `mvi::Pane`), with completion that knows the database's schema, keep them as files, and run them against the database. The first backend is **Cloudflare D1**; local SQLite files work too.

```sh
cargo build --release -p msql
export CLOUDFLARE_API_TOKEN=…        # a token with the "D1 Edit" permission
msql d1 my-database                  # or just `msql d1` inside a project with a wrangler.toml
msql sqlite app.db                   # a local file (needs the sqlite3 command)
```

## Connecting to D1

| Needed | Where it comes from (first found) |
|---|---|
| API token | `$CLOUDFLARE_API_TOKEN` / `$CF_API_TOKEN`, `token …` in `~/.config/msql/config`, then the token `wrangler login` stored (it expires within hours) |
| Account | `$CLOUDFLARE_ACCOUNT_ID`, `account …` in the config, `account_id` in `wrangler.toml`/`wrangler.json[c]`, or the only account the token can see |
| Database | the argument (a name, a wrangler binding, or an id), `d1 NAME` in the config, the only database in the nearest `wrangler.toml`/`wrangler.json[c]`, or the only one in the account |

Queries go to D1's REST API (`…/d1/database/{id}/raw`), which keeps duplicate column names and the column names of empty results. Integers beyond 2⁵³ lose precision (the shared JSON parser holds numbers as `f64`).

## The screen

A schema tree on the left, the editor above, the results below. The schema is cached in `~/.local/share/msql/<database>/` so completion works at once, and refreshed in the background (and after a `CREATE`/`DROP`/`ALTER` you run).

**Writing.** Everything in mvi works. Completion pops up as you type (`Ctrl-Space` opens it by hand, `Tab`/`Ctrl-N`/`Ctrl-P` select, `Enter` accepts) and understands the statement:

* after `FROM`, `JOIN`, `INTO`, `UPDATE`, `DROP TABLE`: tables and views;
* after `u.` (an alias or a table name): that table's columns, even when the `FROM` comes later in the statement;
* inside `INSERT INTO t (`: `t`'s columns;
* elsewhere: the columns of the tables the statement reads, then its aliases, SQLite functions (with signatures) and keywords; before any `FROM`, every column in the schema;
* names that need it are quoted (`"order"`), keywords follow the case you type, and nothing pops up inside strings or comments. The detail line under the popup shows the column's type and `primary key` / `not null`.

**Running.** `Alt-Enter` (or `Ctrl-G`, or `:run`) runs the statement under the cursor — statements are split at `;` outside strings and comments, trigger bodies included — or the visual selection. `:runall` sends the whole buffer. `DELETE`/`UPDATE` without a `WHERE`, and `DROP`, ask first.

**Queries.** The buffer is a file (`scratch.sql`, to begin with) in `~/.local/share/msql/<database>/queries/`. `:w` saves it, `:save NAME` keeps it under a name and switches to it, `:open [NAME]` picks one (fuzzy), `:new [NAME]`, `:rm NAME`. msql reopens the last one.

**Moving.** `Alt-h` schema, `Alt-j` results, `Alt-k`/`Alt-l` editor (or `Ctrl-W h`/`j` in normal mode). `:schema` hides or shows the tree, `:refresh` reloads it. The mouse works: click to focus, wheel to scroll.

**Results.** `h j k l`, `g G 0 $`, `Ctrl-D`/`Ctrl-U`; `Enter` opens the row with one column per line and long values wrapped (`n`/`p` for the next/previous row); `y` copies the cell and `Y` the row (OSC 52, so it works over ssh); `:export out.csv|tsv|json` writes the whole result. NULLs are dim, numbers right-aligned. Errors from the database are shown in full.

**Schema tree.** `Enter` opens a table, `i` inserts the name at the editor's cursor, `p` previews 100 rows, `r` refreshes.

`:msql` shows these keys; `:q` quits.

## Adding a backend

A backend implements `Backend` (`src/backend.rs`): `run(sql) -> Output` and two labels. The schema is read with SQLite's catalog queries by default, so another SQLite-flavoured database only needs `run`; anything else can override `schema()`.
