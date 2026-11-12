// PostgreSQL through the `psql` command. psql brings libpq's authentication,
// TLS, services and .pgpass with it, so any database it can reach works.
//
// Each run is one psql process: the statements go to its stdin, separated by
// marker lines, and come back as CSV (`--csv`, with NULL printed as a
// sentinel so it stays distinct from an empty string). The marker is written
// to stderr too (`\warn`), so the server's notices and errors there can be
// told apart by statement.

use crate::backend::{Backend, Kind, Output, ResultSet, Val};
use crate::config;
use crate::schema::Schema;
use crate::sql;
use std::io::Write;
use std::process::{Command, Stdio};

const NULL: &str = "\u{2400}";
const MARK: &str = "@@msql-statement@@";

pub struct Postgres {
    /// A URI or conninfo string; empty means whatever the PG* variables say.
    pub conn: String,
}

impl Postgres {
    /// The argument, `postgres …` in the config, or `$DATABASE_URL` when it is a Postgres URL.
    pub fn from_args(arg: Option<String>) -> Postgres {
        let cfg = std::fs::read_to_string(config::config_dir().join("config")).ok().and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("postgres ").map(|v| v.trim().to_string())));
        let env = std::env::var("DATABASE_URL").ok().filter(|u| u.starts_with("postgres://") || u.starts_with("postgresql://"));
        Postgres { conn: arg.or(cfg).or(env).unwrap_or_default() }
    }

    fn psql(&self) -> Command {
        let mut c = Command::new("psql");
        c.args(["-X", "-q", "-w", "--csv", "-v", "ON_ERROR_STOP=1", "-P"]).arg(format!("null={}", NULL));
        if !self.conn.is_empty() {
            c.arg(&self.conn);
        }
        c.env("PGAPPNAME", "msql").env("PGCONNECT_TIMEOUT", "10");
        c
    }

    /// host[:port]/database, without credentials.
    fn shown(&self) -> String {
        let c = &self.conn;
        if let Some(rest) = c.strip_prefix("postgres://").or_else(|| c.strip_prefix("postgresql://")) {
            let rest = rest.split('?').next().unwrap_or("");
            return rest.rsplit('@').next().unwrap_or(rest).to_string();
        }
        if c.is_empty() {
            let e = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
            return format!("{}/{}", e("PGHOST").unwrap_or_else(|| "localhost".into()), e("PGDATABASE").or_else(|| e("PGUSER")).unwrap_or_else(|| "postgres".into()));
        }
        // conninfo (`host=… dbname=… password=…`) or a service name: show it without the password
        c.split_whitespace().filter(|w| !w.starts_with("password=")).collect::<Vec<_>>().join(" ")
    }
}

/// RFC 4180 CSV: quoted fields may hold commas, quotes ("") and newlines.
pub fn parse_csv(s: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut was_quoted) = (false, false);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            match c {
                '"' if it.peek() == Some(&'"') => {
                    field.push('"');
                    it.next();
                }
                '"' => quoted = false,
                c => field.push(c),
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                was_quoted = true;
            }
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                was_quoted = false;
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || was_quoted || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn val(s: String) -> Val {
    if s == NULL {
        return Val::null();
    }
    let num = {
        let d = s.strip_prefix('-').unwrap_or(&s);
        !d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.') && d.matches('.').count() <= 1 && !(d.len() > 1 && d.starts_with('0') && !d.starts_with("0."))
    };
    Val { kind: if num { Kind::Num } else { Kind::Text }, text: s }
}

/// One statement's CSV: a header row and the rows, or nothing for a command.
fn set(chunk: &str) -> ResultSet {
    let mut rows = parse_csv(chunk).into_iter();
    let Some(columns) = rows.next() else { return ResultSet::default() };
    ResultSet { columns, rows: rows.map(|r| r.into_iter().map(val).collect()).collect() }
}

/// Part of psql's stderr as one message: `psql:<stdin>:3: ERROR:  …` becomes `ERROR: …`.
fn error_text(stderr: &str) -> String {
    let lines: Vec<String> = stderr.lines().map(|l| l.strip_prefix("psql:<stdin>:").and_then(|r| r.split_once(": ")).map_or(l, |x| x.1).replace("ERROR:  ", "ERROR: ")).collect();
    let t = lines.join("\n").trim().to_string();
    if t.contains("fe_sendauth: no password supplied") || t.contains("no password supplied") {
        return format!("{}\n(msql never prompts: put the password in the URL, $PGPASSWORD or ~/.pgpass)", t);
    }
    t
}

/// psql's stderr as psql would show it, split at the markers into what each
/// statement made the server say. When several statements ran, each one that
/// said something gets a `-- statement N: …` line above it. Anything before the
/// first marker (a failed connection) comes first, as it is.
fn messages(stderr: &str, stmts: &[String]) -> String {
    let mark = format!("{}\n", MARK);
    let mut parts = stderr.split(&mark);
    let mut out = error_text(parts.next().unwrap_or(""));
    for (i, (st, part)) in stmts.iter().zip(parts).enumerate() {
        let t = error_text(part);
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(if stmts.len() > 1 { "\n\n" } else { "\n" });
        }
        if stmts.len() > 1 {
            let first = st.trim().lines().next().unwrap_or("");
            let more = first.chars().count() > 60 || st.trim().contains('\n');
            out.push_str(&format!("-- statement {}: {}{}\n", i + 1, first.chars().take(60).collect::<String>().trim_end(), if more { " …" } else { "" }));
        }
        out.push_str(&t);
    }
    out
}

impl Backend for Postgres {
    fn title(&self) -> String {
        format!("postgres · {}", self.shown())
    }

    fn key(&self) -> String {
        format!("pg-{}", self.shown().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect::<String>())
    }

    fn run(&self, text: &str) -> Result<Output, String> {
        let start = std::time::Instant::now();
        let mut script = String::new();
        let stmts = sql::statements(text);
        for st in &stmts {
            script.push_str(&format!("\\qecho {}\n\\warn {}\n{}\n", MARK, MARK, st));
            if !st.trim_end().ends_with(';') {
                script.push_str(";\n");
            }
        }
        let mut child = self.psql().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("psql: {} (msql uses the psql command)", e))?;
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let _ = writer.join();
        let said = messages(&String::from_utf8_lossy(&out.stderr), &stmts);
        if !out.status.success() {
            return Err(said);
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut sets: Vec<ResultSet> = stdout.split(&format!("{}\n", MARK)).skip(1).map(set).collect();
        sets.truncate(stmts.len());
        Ok(Output { sets, info: format!("{} ms", start.elapsed().as_millis()), messages: said })
    }

    fn schema(&self) -> Result<Schema, String> {
        let q = "SELECT CASE WHEN n.nspname IN ('public', current_schema()) THEN '' ELSE n.nspname END AS sch, c.relname AS tbl, \
                 CASE WHEN c.relkind IN ('v','m') THEN 'view' ELSE 'table' END AS kind, a.attname AS col, format_type(a.atttypid, a.atttypmod) AS ty, \
                 a.attnotnull AS nn, COALESCE(a.attnum = ANY (i.indkey), false) AS pk \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
                 LEFT JOIN pg_index i ON i.indrelid = c.oid AND i.indisprimary \
                 WHERE c.relkind IN ('r','p','v','m','f') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%' \
                 ORDER BY (n.nspname NOT IN ('public', current_schema())), n.nspname, c.relname, a.attnum";
        let out = self.run(q)?;
        let t = |r: &[Val], i: usize| r.get(i).map(|v| v.text.clone()).unwrap_or_default();
        let rows = out.sets.first().map(|s| s.rows.as_slice()).unwrap_or_default();
        Ok(Schema::from_rows(rows.iter().map(|r| (t(r, 0), t(r, 1), t(r, 2) == "view", t(r, 3), t(r, 4), t(r, 5) == "t", t(r, 6) == "t"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quoting_and_nulls() {
        let rows = parse_csv("id,note\n1,\"a, \"\"b\"\"\nc\"\n2,\u{2400}\n3,\n");
        assert_eq!(rows[0], ["id", "note"]);
        assert_eq!(rows[1], ["1", "a, \"b\"\nc"]);
        let s = set("id,note\n1,x\n2,\u{2400}\n3,\n");
        assert_eq!(s.rows[1][1].kind, Kind::Null);
        assert_eq!(s.rows[2][1], Val { text: String::new(), kind: Kind::Text });
        assert_eq!(s.rows[0][0].kind, Kind::Num);
        // zip codes and ids with leading zeros are text
        assert_eq!(val("0123".into()).kind, Kind::Text);
        assert_eq!(val("-1.5".into()).kind, Kind::Num);
        assert!(set("").columns.is_empty());
    }

    #[test]
    fn hides_credentials() {
        let p = Postgres { conn: "postgres://me:secret@db.example.com:5432/app?sslmode=require".into() };
        assert_eq!(p.title(), "postgres · db.example.com:5432/app");
        assert_eq!(p.key(), "pg-db.example.com_5432_app");
        let p = Postgres { conn: "host=h dbname=d password=pw".into() };
        assert_eq!(p.shown(), "host=h dbname=d");
    }

    #[test]
    fn error_cleanup() {
        assert_eq!(error_text("psql:<stdin>:3: ERROR:  relation \"x\" does not exist\nLINE 2: select * from x\n"), "ERROR: relation \"x\" does not exist\nLINE 2: select * from x");
    }

    #[test]
    fn messages_by_statement() {
        let m = |s: &str| format!("{}\n", MARK) + s;
        let stmts: Vec<String> = ["COMMIT;", "SELECT 1;", "SELECT twice();"].iter().map(|s| s.to_string()).collect();
        let stderr = m("WARNING:  there is no transaction in progress\n") + &m("") + &m("ERROR:  relation \"tt\" already exists\nCONTEXT:  SQL statement \"CREATE TEMP TABLE tt(x int)\"\nPL/pgSQL function twice() line 1 at SQL statement\n");
        assert_eq!(
            messages(&stderr, &stmts),
            "-- statement 1: COMMIT;\nWARNING:  there is no transaction in progress\n\n-- statement 3: SELECT twice();\nERROR: relation \"tt\" already exists\nCONTEXT:  SQL statement \"CREATE TEMP TABLE tt(x int)\"\nPL/pgSQL function twice() line 1 at SQL statement"
        );
        // one statement: no heading; a failed connection has no markers at all
        assert_eq!(messages(&m("NOTICE:  table \"nope\" does not exist, skipping\n"), &stmts[..1]), "NOTICE:  table \"nope\" does not exist, skipping");
        assert_eq!(messages("psql: error: connection refused\n", &stmts), "psql: error: connection refused");
    }
}
