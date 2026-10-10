// SQL for the editor: a lexer, statement splitting (which statement is the
// cursor in?) and completion that knows the schema. Completion works out what
// the cursor is looking at (a table name after FROM/JOIN, the columns of an
// alias after `u.`, the columns of whatever tables the statement reads) and
// ranks the schema's names, SQLite's functions and keywords accordingly.

use crate::schema::{Schema, Table};
use mtui::fuzzy::fuzzy;
use mvi::complete::{Completion, Item, Kind};

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    /// A bare word: keyword or identifier.
    Word(String),
    /// "x", `x` or [x], without the quotes.
    Quoted(String),
    Str,
    Num,
    Param,
    Punct(char),
    Comment,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub t: T,
    pub s: usize,
    pub e: usize,
    /// Not terminated: a string or quoted name running to the end of the line
    /// or file, a line comment.
    pub open: bool,
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

pub fn lex(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b: Vec<(usize, char)> = src.char_indices().collect();
    let at = |i: usize| b.get(i).map_or(src.len(), |x| x.0);
    let mut i = 0;
    while i < b.len() {
        let (s, c) = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && b.get(i + 1).map(|x| x.1) == Some('-') {
            while i < b.len() && b[i].1 != '\n' {
                i += 1;
            }
            out.push(Tok { t: T::Comment, s, e: at(i), open: true });
        } else if c == '/' && b.get(i + 1).map(|x| x.1) == Some('*') {
            i += 2;
            while i < b.len() && !(b[i].1 == '*' && b.get(i + 1).map(|x| x.1) == Some('/')) {
                i += 1;
            }
            let open = i >= b.len();
            i = (i + 2).min(b.len());
            out.push(Tok { t: T::Comment, s, e: at(i), open });
        } else if c == '\'' || c == '"' || c == '`' || c == '[' {
            let close = if c == '[' { ']' } else { c };
            let mut j = i + 1;
            let mut name = String::new();
            let mut open = true;
            while j < b.len() {
                let ch = b[j].1;
                if ch == close {
                    if close != ']' && b.get(j + 1).map(|x| x.1) == Some(close) {
                        name.push(close);
                        j += 2;
                        continue;
                    }
                    open = false;
                    j += 1;
                    break;
                }
                // names end at the line; strings may span lines
                if ch == '\n' && c != '\'' {
                    break;
                }
                name.push(ch);
                j += 1;
            }
            out.push(Tok { t: if c == '\'' { T::Str } else { T::Quoted(name) }, s, e: at(j), open });
            i = j;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].1.is_ascii_alphanumeric() || b[j].1 == '.') {
                j += 1;
            }
            out.push(Tok { t: T::Num, s, e: at(j), open: false });
            i = j;
        } else if word_char(c) {
            let mut j = i;
            while j < b.len() && word_char(b[j].1) {
                j += 1;
            }
            out.push(Tok { t: T::Word(src[s..at(j)].to_string()), s, e: at(j), open: false });
            i = j;
        } else if (c == ':' || c == '@' || c == '$' || c == '?') && b.get(i + 1).is_some_and(|x| word_char(x.1) || c == '?') {
            let mut j = i + 1;
            while j < b.len() && word_char(b[j].1) {
                j += 1;
            }
            out.push(Tok { t: T::Param, s, e: at(j), open: false });
            i = j;
        } else {
            out.push(Tok { t: T::Punct(c), s, e: at(i + 1), open: false });
            i += 1;
        }
    }
    out
}

fn kw(t: &Tok) -> Option<String> {
    match &t.t {
        T::Word(w) => Some(w.to_ascii_uppercase()),
        _ => None,
    }
}

fn name_of(t: &Tok) -> Option<&str> {
    match &t.t {
        T::Word(w) | T::Quoted(w) => Some(w),
        _ => None,
    }
}

/// Token index ranges of the statements: `;` ends one, except inside a
/// trigger body (CREATE TRIGGER … BEGIN … ; … END) and CASE … END.
pub fn split(toks: &[Tok]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut depth, mut trigger) = (0, 0i32, false);
    let mut first_words: Vec<String> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.t == T::Comment {
            continue;
        }
        if start == i || first_words.len() < 4 {
            if let Some(w) = kw(t) {
                first_words.push(w);
            }
            trigger = first_words.first().is_some_and(|w| w == "CREATE") && first_words.iter().take(3).any(|w| w == "TRIGGER");
        }
        match kw(t).as_deref() {
            Some("CASE") => depth += 1,
            Some("BEGIN") if trigger => depth += 1,
            Some("END") if depth > 0 => depth -= 1,
            _ => {}
        }
        if t.t == T::Punct(';') && depth == 0 {
            out.push(start..i + 1);
            start = i + 1;
            first_words.clear();
            trigger = false;
        }
    }
    if start < toks.len() {
        out.push(start..toks.len());
    }
    out
}

/// The text of the statement the cursor (a byte offset) is in, with its
/// start offset. Between statements it is the one before the cursor.
pub fn statement_at(src: &str, off: usize) -> Option<(usize, String)> {
    let toks = lex(src);
    let mut best: Option<(usize, usize, usize)> = None; // (distance, s, e)
    for r in split(&toks) {
        let real: Vec<&Tok> = toks[r.clone()].iter().filter(|t| t.t != T::Comment).collect();
        let (Some(f), Some(l)) = (real.first(), real.last()) else { continue };
        if real.len() == 1 && l.t == T::Punct(';') {
            continue;
        }
        let d = if off < f.s { f.s - off } else { off.saturating_sub(l.e) };
        if best.map_or(true, |b| d < b.0) {
            best = Some((d, f.s, l.e));
        }
    }
    best.map(|(_, s, e)| (s, src[s..e].to_string()))
}

/// Every statement's text, comments dropped, empty ones skipped.
pub fn statements(src: &str) -> Vec<String> {
    let toks = lex(src);
    split(&toks)
        .into_iter()
        .filter_map(|r| {
            let real: Vec<&Tok> = toks[r].iter().filter(|t| t.t != T::Comment).collect();
            let (f, l) = (real.first()?, real.last()?);
            (!(real.len() == 1 && l.t == T::Punct(';'))).then(|| src[f.s..l.e].to_string())
        })
        .collect()
}

pub const KEYWORDS: &[&str] = &[
    "ABORT", "ADD", "ALL", "ALTER", "ANALYZE", "AND", "AS", "ASC", "ATTACH", "AUTOINCREMENT", "BEGIN", "BETWEEN", "BY", "CASCADE", "CASE", "CAST", "CHECK", "COLLATE", "COLUMN", "COMMIT", "CONFLICT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT",
    "DEFAULT", "DEFERRABLE", "DELETE", "DESC", "DETACH", "DISTINCT", "DO", "DROP", "ELSE", "END", "ESCAPE", "EXCEPT", "EXISTS", "EXPLAIN", "FAIL", "FILTER", "FIRST", "FOLLOWING", "FOREIGN", "FROM", "FULL", "GLOB", "GROUP", "HAVING", "IF",
    "IGNORE", "IMMEDIATE", "IN", "INDEX", "INDEXED", "INNER", "INSERT", "INSTEAD", "INTERSECT", "INTO", "IS", "ISNULL", "JOIN", "KEY", "LAST", "LEFT", "LIKE", "LIMIT", "MATCH", "NATURAL", "NOT", "NOTHING", "NOTNULL", "NULL", "NULLS", "OFFSET",
    "ON", "OR", "ORDER", "OUTER", "OVER", "PARTITION", "PRAGMA", "PRECEDING", "PRIMARY", "RANGE", "RECURSIVE", "REFERENCES", "REGEXP", "REINDEX", "RELEASE", "RENAME", "REPLACE", "RESTRICT", "RETURNING", "RIGHT", "ROLLBACK", "ROW", "ROWS",
    "SAVEPOINT", "SELECT", "SET", "TABLE", "TEMP", "TEMPORARY", "THEN", "TO", "TRANSACTION", "TRIGGER", "UNBOUNDED", "UNION", "UNIQUE", "UPDATE", "USING", "VACUUM", "VALUES", "VIEW", "VIRTUAL", "WHEN", "WHERE", "WINDOW", "WITH", "WITHOUT",
];

/// Words that must be quoted to be used as a name.
const RESERVED: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "AS", "BETWEEN", "BY", "CASE", "CHECK", "COLLATE", "COLUMN", "COMMIT", "CONSTRAINT", "CREATE", "CROSS", "DEFAULT", "DELETE", "DESC", "DISTINCT", "DROP", "ELSE", "END", "ESCAPE", "EXCEPT", "EXISTS", "FOREIGN", "FROM",
    "FULL", "GROUP", "HAVING", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN", "KEY", "LEFT", "LIKE", "LIMIT", "NOT", "NULL", "ON", "OR", "ORDER", "OUTER", "PRIMARY", "REFERENCES", "RIGHT", "SELECT", "SET", "TABLE", "THEN", "TO",
    "UNION", "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW", "WHEN", "WHERE", "WITH",
];

/// (name, signature) of SQLite's common functions.
pub const FUNCTIONS: &[(&str, &str)] = &[
    ("abs", "abs(x)"),
    ("avg", "avg(x)"),
    ("cast", "cast(x AS type)"),
    ("changes", "changes()"),
    ("char", "char(x1, x2, …)"),
    ("coalesce", "coalesce(x, y, …)"),
    ("count", "count(x) | count(*)"),
    ("date", "date(time, modifier, …)"),
    ("datetime", "datetime(time, modifier, …)"),
    ("dense_rank", "dense_rank() OVER (…)"),
    ("first_value", "first_value(x) OVER (…)"),
    ("glob", "glob(pattern, x)"),
    ("group_concat", "group_concat(x, sep)"),
    ("hex", "hex(blob)"),
    ("ifnull", "ifnull(x, y)"),
    ("iif", "iif(cond, a, b)"),
    ("instr", "instr(x, y)"),
    ("json", "json(text)"),
    ("json_array", "json_array(x, …)"),
    ("json_array_length", "json_array_length(json, path)"),
    ("json_each", "json_each(json, path)"),
    ("json_extract", "json_extract(json, path, …)"),
    ("json_group_array", "json_group_array(x)"),
    ("json_group_object", "json_group_object(name, value)"),
    ("json_insert", "json_insert(json, path, value, …)"),
    ("json_object", "json_object(label, value, …)"),
    ("json_remove", "json_remove(json, path, …)"),
    ("json_set", "json_set(json, path, value, …)"),
    ("json_tree", "json_tree(json, path)"),
    ("json_type", "json_type(json, path)"),
    ("julianday", "julianday(time, modifier, …)"),
    ("lag", "lag(x, offset, default) OVER (…)"),
    ("last_insert_rowid", "last_insert_rowid()"),
    ("last_value", "last_value(x) OVER (…)"),
    ("lead", "lead(x, offset, default) OVER (…)"),
    ("length", "length(x)"),
    ("like", "like(pattern, x)"),
    ("lower", "lower(x)"),
    ("ltrim", "ltrim(x, chars)"),
    ("max", "max(x, …)"),
    ("min", "min(x, …)"),
    ("nullif", "nullif(x, y)"),
    ("printf", "printf(format, …)"),
    ("quote", "quote(x)"),
    ("random", "random()"),
    ("rank", "rank() OVER (…)"),
    ("replace", "replace(x, find, with)"),
    ("round", "round(x, digits)"),
    ("row_number", "row_number() OVER (…)"),
    ("rtrim", "rtrim(x, chars)"),
    ("strftime", "strftime(format, time, modifier, …)"),
    ("substr", "substr(x, start, length)"),
    ("sum", "sum(x)"),
    ("time", "time(time, modifier, …)"),
    ("total", "total(x)"),
    ("trim", "trim(x, chars)"),
    ("typeof", "typeof(x)"),
    ("unixepoch", "unixepoch(time, modifier, …)"),
    ("upper", "upper(x)"),
    ("zeroblob", "zeroblob(n)"),
];

/// Words that can follow a table name without being its alias.
const NOT_ALIAS: &[&str] = &[
    "WHERE", "GROUP", "ORDER", "LIMIT", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "CROSS", "NATURAL", "FULL", "ON", "USING", "SET", "VALUES", "SELECT", "UNION", "EXCEPT", "INTERSECT", "HAVING", "WINDOW", "RETURNING", "DEFAULT", "INDEXED", "NOT", "OFFSET",
    "WHEN", "DO", "AS",
];

/// Words that start a clause; the last one before the cursor says what is expected.
const CLAUSES: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "ON", "USING", "SET", "VALUES", "JOIN", "INTO", "UPDATE", "RETURNING", "UNION", "EXCEPT", "INTERSECT", "WITH", "BY", "DELETE", "INSERT", "TABLE", "VIEW", "INDEX", "DISTINCT",
];

/// What a statement reads from: (alias or name, table name — empty when unknown, e.g. a subquery).
fn scope(toks: &[&Tok]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let k = kw(toks[i]).unwrap_or_default();
        if !matches!(k.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            // CTE: name AS (
            if let (Some(n), Some(a), Some(p)) = (name_of(toks[i]), toks.get(i + 1).and_then(|t| kw(t)), toks.get(i + 2)) {
                let before_ok = i == 0 || matches!(toks[i - 1].t, T::Punct(',')) || kw(toks[i - 1]).is_some_and(|w| w == "WITH" || w == "RECURSIVE");
                if a == "AS" && p.t == T::Punct('(') && before_ok && kw(toks[i]).map_or(true, |w| !NOT_ALIAS.contains(&w.as_str())) {
                    out.push((n.to_string(), String::new()));
                }
            }
            i += 1;
            continue;
        }
        i += 1;
        if k == "UPDATE" && toks.get(i).and_then(|t| kw(t)).is_some_and(|w| w == "OR") {
            i += 2;
        }
        loop {
            // a table (optionally schema-qualified), a subquery, or nothing
            let mut table = String::new();
            if toks.get(i).is_some_and(|t| t.t == T::Punct('(')) {
                let mut d = 0;
                while i < toks.len() {
                    match toks[i].t {
                        T::Punct('(') => d += 1,
                        T::Punct(')') => {
                            d -= 1;
                            if d == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            } else if let Some(n) = toks.get(i).and_then(|t| name_of(t)).filter(|_| toks.get(i).and_then(|t| kw(t)).map_or(true, |w| !NOT_ALIAS.contains(&w.as_str()) && w != "ONLY")) {
                table = n.to_string();
                i += 1;
                if toks.get(i).is_some_and(|t| t.t == T::Punct('.')) {
                    if let Some(n2) = toks.get(i + 1).and_then(|t| name_of(t)) {
                        table = n2.to_string();
                        i += 2;
                    }
                }
            } else {
                break;
            }
            let mut alias = None;
            if toks.get(i).and_then(|t| kw(t)).is_some_and(|w| w == "AS") {
                i += 1;
            }
            if let Some(a) = toks.get(i).and_then(|t| name_of(t)) {
                if toks[i].t != T::Word(a.to_string()) || !NOT_ALIAS.contains(&a.to_ascii_uppercase().as_str()) {
                    alias = Some(a.to_string());
                    i += 1;
                }
            }
            match (alias, table.is_empty()) {
                (Some(a), _) => out.push((a, table)),
                (None, false) => out.push((table.clone(), table)),
                _ => {}
            }
            // FROM a, b
            if k == "FROM" && toks.get(i).is_some_and(|t| t.t == T::Punct(',')) {
                i += 1;
                continue;
            }
            break;
        }
    }
    out
}

/// A name as it must be written in SQL.
pub fn ident(name: &str) -> String {
    let simple = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if simple && !RESERVED.contains(&name.to_ascii_uppercase().as_str()) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn col_detail(t: &Table, c: &crate::schema::Column) -> String {
    let mut s = format!("{}.{}", t.name, c.name);
    if !c.ty.is_empty() {
        s.push_str(&format!(" · {}", c.ty));
    }
    if c.pk {
        s.push_str(" · primary key");
    }
    if c.notnull {
        s.push_str(" · not null");
    }
    s
}

#[derive(PartialEq)]
enum Want {
    /// Only keywords: the start of a statement.
    Start,
    /// A table or view name.
    Table,
    /// The columns of these tables only (`u.`, `INSERT INTO t (`).
    Columns(Vec<String>),
    /// An expression: columns in scope, functions, keywords.
    Expr,
}

struct Cand {
    text: String,
    kind: Kind,
    detail: String,
    group: i32,
}

pub fn complete(schema: &Schema, lines: &[String], cur: (usize, usize), manual: bool) -> Option<Completion> {
    let src = lines.join("\n");
    let off = lines.iter().take(cur.0).map(|l| l.len() + 1).sum::<usize>() + cur.1;
    let toks = lex(&src);
    // inside a string or a comment: nothing to offer
    if toks.iter().any(|t| matches!(t.t, T::Str | T::Comment) && t.s < off && (off < t.e || (off == t.e && t.open))) {
        return None;
    }
    // the word being typed; or a quoted name still open at the cursor
    let (pstart, quoted) = match toks.iter().find(|t| matches!(t.t, T::Quoted(_)) && t.open && t.s < off && off <= t.e) {
        Some(t) => (t.s, true),
        None => (src[..off].char_indices().rev().take_while(|&(_, c)| word_char(c)).last().map_or(off, |x| x.0), false),
    };
    let prefix = if quoted { &src[pstart + 1..off] } else { &src[pstart..off] };
    if prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }

    // the statement holding the cursor: after the last `;` that ends before it
    let a = toks.iter().take_while(|t| t.e <= pstart).enumerate().filter(|(_, t)| t.t == T::Punct(';')).map(|(i, _)| i + 1).last().unwrap_or(0);
    let b = toks.iter().enumerate().skip(a).find(|(_, t)| t.s >= off && t.t == T::Punct(';')).map_or(toks.len(), |(i, _)| i);
    let stmt: Vec<&Tok> = toks[a..b].iter().filter(|t| t.t != T::Comment).collect();
    let before: Vec<&Tok> = stmt.iter().copied().filter(|t| t.e <= pstart).collect();
    let sc = scope(&stmt);

    let resolve = |q: &str| -> Option<&Table> {
        let tn = sc.iter().find(|(a, _)| a.eq_ignore_ascii_case(q)).map(|(_, t)| t.as_str()).unwrap_or(q);
        schema.find(tn)
    };

    // what is expected here
    let prev = before.last();
    let prev_kw = prev.and_then(|t| kw(t)).unwrap_or_default();
    let mut dotted = false;
    let want = if prev.is_some_and(|t| t.t == T::Punct('.')) {
        dotted = true;
        match before.len().checked_sub(2).and_then(|i| name_of(before[i])) {
            Some(q) if matches!(q.to_ascii_lowercase().as_str(), "main" | "temp") && resolve(q).is_none() => Want::Table,
            Some(q) => Want::Columns(resolve(q).map(|t| vec![t.name.clone()]).unwrap_or_default()),
            None => return None,
        }
    } else if before.is_empty() {
        Want::Start
    } else if matches!(prev_kw.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE")
        || (prev_kw == "TABLE" && before.len() >= 2 && matches!(kw(before[before.len() - 2]).as_deref(), Some("DROP" | "ALTER")))
        || (prev_kw == "VIEW" && before.len() >= 2 && kw(before[before.len() - 2]).as_deref() == Some("DROP"))
        || (prev_kw == "EXISTS" && before.len() >= 3 && matches!(kw(before[before.len() - 3]).as_deref(), Some("TABLE" | "VIEW")))
    {
        Want::Table
    } else {
        // clause stack: the innermost open clause, and INSERT INTO t ( column lists
        let mut stack: Vec<(Option<String>, usize)> = vec![(None, 0)];
        for (i, t) in before.iter().enumerate() {
            match t.t {
                T::Punct('(') => stack.push((None, i)),
                T::Punct(')') if stack.len() > 1 => {
                    stack.pop();
                }
                _ => {
                    if let Some(w) = kw(t).filter(|w| CLAUSES.contains(&w.as_str())) {
                        stack.last_mut().unwrap().0 = Some(w);
                    }
                }
            }
        }
        let (clause, open) = stack.last().cloned().unwrap();
        if matches!(prev.map(|t| &t.t), Some(T::Punct(','))) && clause.as_deref() == Some("FROM") {
            Want::Table
        } else if clause.is_none() && stack.len() > 1 {
            // `INSERT INTO t (` and `INSERT INTO s.t (`
            let n = open.checked_sub(1).and_then(|i| name_of(before[i]));
            let into = (1..=3).any(|d| open.checked_sub(d).and_then(|i| before.get(i)).is_some_and(|t| kw(t).as_deref() == Some("INTO")));
            match (n, into) {
                (Some(n), true) => Want::Columns(vec![n.to_string()]),
                _ => Want::Expr,
            }
        } else {
            Want::Expr
        }
    };

    // when to pop up on its own
    let min = match want {
        Want::Columns(_) if dotted => 0,
        Want::Table | Want::Columns(_) => 1,
        _ => 2,
    };
    if !manual && prefix.chars().count() < min {
        return None;
    }
    if !manual && matches!(want, Want::Columns(ref v) if v.is_empty()) {
        return None;
    }

    let lower = !prefix.is_empty() && !prefix.chars().any(|c| c.is_uppercase());
    let case = |w: &str| if lower { w.to_ascii_lowercase() } else { w.to_string() };
    let mut cands: Vec<Cand> = Vec::new();
    let table_cands = |cands: &mut Vec<Cand>, group: i32| {
        for t in &schema.tables {
            cands.push(Cand { text: ident(&t.name), kind: Kind::Table, detail: format!("{} · {} columns", if t.view { "view" } else { "table" }, t.columns.len()), group });
        }
        for (a, t) in &sc {
            if t.is_empty() && !schema.tables.iter().any(|x| x.name == *a) {
                cands.push(Cand { text: ident(a), kind: Kind::Table, detail: "subquery / CTE".into(), group });
            }
        }
    };
    let col_cands = |cands: &mut Vec<Cand>, t: &Table, group: i32| {
        for c in &t.columns {
            cands.push(Cand { text: ident(&c.name), kind: Kind::Column, detail: col_detail(t, c), group });
        }
    };
    match &want {
        Want::Start => {
            for k in KEYWORDS.iter().filter(|k| matches!(**k, "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "CREATE" | "DROP" | "ALTER" | "WITH" | "PRAGMA" | "EXPLAIN" | "BEGIN" | "COMMIT" | "ROLLBACK" | "VACUUM" | "ANALYZE" | "REPLACE")) {
                cands.push(Cand { text: case(k), kind: Kind::Keyword, detail: String::new(), group: 100 });
            }
        }
        Want::Table => table_cands(&mut cands, 100),
        Want::Columns(ts) => {
            for t in ts.iter().filter_map(|n| schema.find(n)) {
                col_cands(&mut cands, t, 100);
            }
        }
        Want::Expr => {
            let mut seen_tables = Vec::new();
            for (a, tn) in &sc {
                if let Some(t) = schema.find(tn) {
                    if !seen_tables.contains(&t.name) {
                        seen_tables.push(t.name.clone());
                        col_cands(&mut cands, t, 100);
                    }
                    if !a.eq_ignore_ascii_case(&t.name) {
                        cands.push(Cand { text: ident(a), kind: Kind::Table, detail: format!("alias of {}", t.name), group: 80 });
                    }
                }
            }
            for (a, tn) in &sc {
                if schema.find(tn).is_some() && a.eq_ignore_ascii_case(tn) {
                    cands.push(Cand { text: ident(a), kind: Kind::Table, detail: "table in this query".into(), group: 80 });
                }
            }
            for (name, sig) in FUNCTIONS {
                cands.push(Cand { text: format!("{}(", case(name)), kind: Kind::Symbol, detail: sig.to_string(), group: 60 });
            }
            for k in KEYWORDS {
                cands.push(Cand { text: case(k), kind: Kind::Keyword, detail: String::new(), group: 50 });
            }
            if sc.is_empty() {
                // nothing to scope to yet (SELECT before FROM): offer every column, then the tables
                for t in &schema.tables {
                    col_cands(&mut cands, t, 70);
                }
            }
            table_cands(&mut cands, 10);
        }
    }

    let lp = prefix.to_lowercase();
    let mut scored: Vec<(i32, Cand)> = Vec::new();
    for c in cands {
        let bare = c.text.trim_matches('"').trim_end_matches('(');
        if bare.eq_ignore_ascii_case(prefix) && !prefix.is_empty() && c.kind != Kind::Symbol {
            continue;
        }
        let m = if bare.to_lowercase().starts_with(&lp) {
            2000 + if bare.starts_with(prefix) { 100 } else { 0 }
        } else if prefix.chars().count() >= 2 {
            match fuzzy(prefix, bare) {
                Some(s) => 1000 + s.min(900),
                None => continue,
            }
        } else {
            continue;
        };
        scored.push((m * 100 + c.group * 10 - (c.text.len() as i32).min(9), c));
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.text.cmp(&b.1.text)));
    scored.dedup_by(|a, b| a.1.text == b.1.text && a.1.detail == b.1.detail);
    scored.truncate(60);
    let items = scored
        .into_iter()
        .map(|(_, c)| {
            // a quoted-name prefix is replaced along with its quote
            let text = if quoted && !c.text.starts_with('"') && c.kind != Kind::Symbol && c.kind != Kind::Keyword { format!("\"{}\"", c.text) } else { c.text };
            Item { text, kind: c.kind, detail: c.detail }
        })
        .collect();
    // `start` is a byte column in the cursor's line
    let line_start = off - cur.1;
    Some(Completion { items, sel: None, start: pstart.saturating_sub(line_start), line: cur.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::from_rows(
            [
                ("users", false, "id", "INTEGER", true, true),
                ("users", false, "email", "TEXT", true, false),
                ("users", false, "created_at", "TEXT", false, false),
                ("orders", false, "id", "INTEGER", true, true),
                ("orders", false, "user_id", "INTEGER", true, false),
                ("orders", false, "total", "REAL", false, false),
                ("order", false, "x", "", false, false),
            ]
            .map(|(t, v, c, ty, n, p)| (t.to_string(), v, c.to_string(), ty.to_string(), n, p)),
        )
    }

    /// Complete at the `|` in `text`.
    fn at(text: &str, manual: bool) -> Vec<String> {
        let off = text.find('|').unwrap();
        let src = text.replace('|', "");
        let lines: Vec<String> = src.split('\n').map(String::from).collect();
        let before = &src[..off];
        let cy = before.matches('\n').count();
        let cx = off - before.rfind('\n').map_or(0, |i| i + 1);
        complete(&schema(), &lines, (cy, cx), manual).map(|c| c.items.into_iter().map(|i| i.text).collect()).unwrap_or_default()
    }

    #[test]
    fn splits_statements_and_triggers() {
        let s = "select 1; select ';' ; -- c;\nselect 2";
        assert_eq!(statements(s), ["select 1;", "select ';' ;", "select 2"]);
        let t = "create trigger t after insert on a begin update b set x = case when 1 then 2 end; delete from c; end; select 1";
        assert_eq!(statements(t).len(), 2);
    }

    #[test]
    fn statement_at_cursor() {
        let s = "select 1;\nselect 2;\n\nselect 3";
        assert_eq!(statement_at(s, 3).unwrap().1, "select 1;");
        assert_eq!(statement_at(s, 12).unwrap().1, "select 2;");
        assert_eq!(statement_at(s, s.len()).unwrap().1, "select 3");
        assert_eq!(statement_at(s, 19).unwrap().1, "select 2;");
        assert_eq!(statement_at("  \n", 1), None);
    }

    #[test]
    fn tables_after_from_and_join() {
        let r = at("select * from us|", false);
        assert_eq!(r.first().map(String::as_str), Some("users"));
        let r = at("select * from users u join or|", false);
        assert!(r.contains(&"orders".to_string()) && !r.contains(&"email".to_string()), "{:?}", r);
        assert!(at("select * from users, ord|", false).contains(&"orders".to_string()));
        assert!(at("select * from |", true).len() >= 3);
        // reserved names are quoted
        assert!(at("select * from ord|", false).contains(&"\"order\"".to_string()));
    }

    #[test]
    fn columns_of_an_alias() {
        let r = at("select u.| from users u", false);
        assert_eq!(r.len(), 3, "{:?}", r);
        assert!(r.contains(&"email".to_string()));
        let r = at("select o.to| from users u join orders o on o.user_id = u.id", false);
        assert_eq!(r, ["total"]);
        assert_eq!(at("select users.em| from users", false), ["email"]);
        assert!(at("select x.| from users", false).is_empty());
    }

    #[test]
    fn columns_of_the_tables_in_scope() {
        let r = at("select ema| from users", false);
        assert_eq!(r.first().map(String::as_str), Some("email"));
        // before FROM is written, every table's columns are offered
        let r = at("select tot|", false);
        assert_eq!(r.first().map(String::as_str), Some("total"));
        // only the joined tables' columns come first
        let r = at("select * from orders where us|", false);
        assert_eq!(r.first().map(String::as_str), Some("user_id"));
        // keywords follow the case typed
        assert!(at("select * from users whe|", false).contains(&"where".to_string()));
        assert!(at("select * from users WHE|", false).contains(&"WHERE".to_string()));
    }

    #[test]
    fn insert_column_lists_and_statement_isolation() {
        let r = at("insert into users (|", true);
        assert_eq!(r.len(), 3);
        // a finished statement above doesn't leak its tables in
        let r = at("select * from orders;\nselect ema| from users", false);
        assert_eq!(r.first().map(String::as_str), Some("email"));
        let r = at("select * from orders;\nselect to|", false);
        assert_eq!(r.first().map(String::as_str), Some("total"));
    }

    #[test]
    fn nothing_in_strings_comments_or_numbers() {
        assert!(at("select 'us|", false).is_empty());
        assert!(at("select 1 -- us|", false).is_empty());
        assert!(at("select 12|", false).is_empty());
        // quoted names complete, quotes and all
        let r = at("select \"em| from users", false);
        assert_eq!(r.first().map(String::as_str), Some("\"email\""));
    }

    #[test]
    fn start_of_statement_offers_verbs() {
        let r = at("sel|", false);
        assert_eq!(r.first().map(String::as_str), Some("select"));
    }

    #[test]
    fn completion_start_is_a_column_in_the_line() {
        let src = vec!["select 1;".to_string(), "select users.em".to_string()];
        let c = complete(&schema(), &src, (1, 14), false).unwrap();
        assert_eq!((c.line, c.start), (1, 13));
    }
}
