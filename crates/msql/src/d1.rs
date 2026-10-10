// Cloudflare D1 over its REST API: POST /accounts/{account}/d1/database/{db}/raw
// with a bearer token. `raw` returns column names and rows as arrays, so
// duplicate and empty-result columns survive (the plain /query endpoint
// returns objects and loses both).

use crate::backend::{Backend, Kind, Output, ResultSet, Val};
use crate::config;
use mhttp::Client;
use mtui::json::{self, Value};
use std::sync::Mutex;

const API: &str = "https://api.cloudflare.com/client/v4";

pub struct D1 {
    token: String,
    account: String,
    pub id: String,
    pub name: String,
    /// Idle keep-alive connections, so a schema refresh and a query don't wait on each other.
    pool: Mutex<Vec<Client>>,
}

/// Where the `error` of a D1 reply lives: `{"errors": [{"code": 7500, "message": "..."}]}`.
fn error_text(body: &Value, status: u16) -> String {
    let msgs: Vec<String> = body.get("errors").arr().iter().map(|e| e.get("message").str().to_string()).filter(|s| !s.is_empty()).collect();
    if !msgs.is_empty() {
        return msgs.join("; ");
    }
    match status {
        401 | 403 => "HTTP 401/403: the API token is missing, expired or lacks D1 edit access (wrangler login tokens expire; an API token with the 'D1 Edit' permission does not)".into(),
        404 => "HTTP 404: no such account or database".into(),
        s => format!("HTTP {}", s),
    }
}

fn get(token: &str, path: &str) -> Result<Value, String> {
    let r = Client::new().request("GET", &format!("{}{}", API, path), &[("Authorization", &format!("Bearer {}", token))], b"")?;
    let body = json::parse(&String::from_utf8_lossy(&r.body)).unwrap_or(Value::Null);
    if r.status >= 300 || !matches!(body.get("success"), Value::Bool(true)) {
        return Err(error_text(&body, r.status));
    }
    Ok(body)
}

/// The accounts a token can see: (id, name).
pub fn accounts(token: &str) -> Result<Vec<(String, String)>, String> {
    Ok(get(token, "/accounts?per_page=50")?.get("result").arr().iter().map(|a| (a.get("id").str().to_string(), a.get("name").str().to_string())).collect())
}

/// The D1 databases of an account: (uuid, name).
pub fn databases(token: &str, account: &str) -> Result<Vec<(String, String)>, String> {
    Ok(get(token, &format!("/accounts/{}/d1/database?per_page=100", account))?.get("result").arr().iter().map(|d| (d.get("uuid").str().to_string(), d.get("name").str().to_string())).collect())
}

impl D1 {
    pub fn new(token: String, account: String, id: String, name: String) -> D1 {
        D1 { token, account, id, name, pool: Mutex::new(Vec::new()) }
    }

    /// Read the environment, `~/.config/msql/config` and wrangler's files, then look up what is missing.
    pub fn connect(want: Option<String>) -> Result<D1, String> {
        let c = config::discover(want);
        let token = c.token.ok_or("no Cloudflare API token: set CLOUDFLARE_API_TOKEN (a token with the 'D1 Edit' permission), add `token …` to ~/.config/msql/config, or run `wrangler login`")?;
        let account = match c.account {
            Some(a) => a,
            None => {
                let mut v = accounts(&token).map_err(|e| format!("can't find the account ({}); set CLOUDFLARE_ACCOUNT_ID", e))?;
                match v.len() {
                    0 => return Err("this token sees no accounts; set CLOUDFLARE_ACCOUNT_ID".into()),
                    1 => v.remove(0).0,
                    _ => return Err(format!("several accounts; set CLOUDFLARE_ACCOUNT_ID to one of:\n{}", v.iter().map(|(i, n)| format!("  {}  {}", i, n)).collect::<Vec<_>>().join("\n"))),
                }
            }
        };
        let list = |why: &str| -> Result<Vec<(String, String)>, String> { databases(&token, &account).map_err(|e| format!("can't list databases ({}); {}", e, why)) };
        let (id, name) = match c.db {
            Some(w) if config::is_uuid(&w) => (w.clone(), c.names.iter().find(|(i, _)| *i == w).map(|(_, n)| n.clone()).unwrap_or(w)),
            Some(w) => match c.names.iter().find(|(_, n)| *n == w) {
                Some((i, n)) => (i.clone(), n.clone()),
                None => {
                    let v = list("pass the database id instead")?;
                    match v.iter().find(|(_, n)| *n == w) {
                        Some((i, n)) => (i.clone(), n.clone()),
                        None => return Err(format!("no database named {}; the account has:\n{}", w, v.iter().map(|(i, n)| format!("  {}  {}", n, i)).collect::<Vec<_>>().join("\n"))),
                    }
                }
            },
            None => {
                let v = if c.names.len() == 1 { c.names.clone() } else { list("pass a database name")? };
                match v.len() {
                    0 => return Err("the account has no D1 databases".into()),
                    1 => (v[0].0.clone(), v[0].1.clone()),
                    _ => return Err(format!("which database? msql d1 NAME, one of:\n{}", v.iter().map(|(i, n)| format!("  {}  {}", n, i)).collect::<Vec<_>>().join("\n"))),
                }
            }
        };
        Ok(D1::new(token, account, id, name))
    }

    fn post(&self, endpoint: &str, sql: &str) -> Result<(u16, Value), String> {
        let mut c = self.pool.lock().unwrap().pop().unwrap_or_default();
        let url = format!("{}/accounts/{}/d1/database/{}/{}", API, self.account, self.id, endpoint);
        let auth = format!("Bearer {}", self.token);
        let r = c.request("POST", &url, &[("Authorization", &auth), ("Content-Type", "application/json")], format!("{{\"sql\":{}}}", json::quote(sql)).as_bytes());
        if r.is_ok() {
            self.pool.lock().unwrap().push(c);
        }
        let r = r?;
        Ok((r.status, json::parse(&String::from_utf8_lossy(&r.body)).unwrap_or(Value::Null)))
    }
}

/// A JSON value as a cell. Numbers lose precision beyond 2^53 (the JSON
/// parser holds f64), blobs and nested values show as JSON.
pub fn cell(v: &Value) -> Val {
    match v {
        Value::Null => Val::null(),
        Value::Num(n) => Val { text: num(*n), kind: Kind::Num },
        Value::Str(s) => Val { text: s.clone(), kind: Kind::Text },
        Value::Bool(b) => Val { text: (*b as u8).to_string(), kind: Kind::Num },
        other => Val { text: dump(other), kind: Kind::Text },
    }
}

fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

fn dump(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Num(n) => num(*n),
        Value::Str(s) => json::quote(s),
        Value::Arr(a) => format!("[{}]", a.iter().map(dump).collect::<Vec<_>>().join(",")),
        Value::Obj(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}:{}", json::quote(k), dump(v))).collect::<Vec<_>>().join(",")),
    }
}

/// One statement's reply: `results` is `{columns, rows}` for /raw and an
/// array of row objects for /query.
fn set(r: &Value) -> ResultSet {
    let res = r.get("results");
    match res {
        Value::Obj(_) => ResultSet { columns: res.get("columns").arr().iter().map(|c| c.str().to_string()).collect(), rows: res.get("rows").arr().iter().map(|row| row.arr().iter().map(cell).collect()).collect() },
        _ => set_of_rows(res),
    }
}

/// An array of row objects (the first row's keys are the columns).
pub fn set_of_rows(res: &Value) -> ResultSet {
    let rows = res.arr();
    let columns: Vec<String> = match rows.first() {
        Some(Value::Obj(o)) => o.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    };
    // by position, so repeated column names keep their own values
    let value = |row: &Value, i: usize| match row {
        Value::Obj(o) => o.get(i).map_or_else(Val::null, |(_, v)| cell(v)),
        _ => Val::null(),
    };
    ResultSet { rows: rows.iter().map(|row| (0..columns.len()).map(|i| value(row, i)).collect()).collect(), columns }
}

fn info(results: &[Value]) -> String {
    let (mut ms, mut read, mut wrote, mut changes) = (0.0, 0.0, 0.0, 0.0);
    for r in results {
        let m = r.get("meta");
        ms += m.get("duration").num();
        read += m.get("rows_read").num();
        wrote += m.get("rows_written").num();
        changes += m.get("changes").num();
    }
    let mut parts = vec![format!("{:.1} ms", ms)];
    if read > 0.0 {
        parts.push(format!("read {}", read as u64));
    }
    if wrote > 0.0 {
        parts.push(format!("wrote {}", wrote as u64));
    }
    if changes > 0.0 {
        parts.push(format!("{} changed", changes as u64));
    }
    parts.join(" · ")
}

impl Backend for D1 {
    fn title(&self) -> String {
        format!("d1 · {}", self.name)
    }

    fn key(&self) -> String {
        let safe: String = self.name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        format!("d1-{}", safe)
    }

    fn run(&self, sql: &str) -> Result<Output, String> {
        let (status, body) = self.post("raw", sql)?;
        if status >= 300 || !matches!(body.get("success"), Value::Bool(true)) {
            return Err(error_text(&body, status));
        }
        let results = body.get("result").arr();
        Ok(Output { sets: results.iter().map(set).collect(), info: info(results) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_and_query_shapes() {
        let raw = json::parse(r#"{"results":{"columns":["id","id","n"],"rows":[[1,2,null],[3,4,"x"]]},"meta":{"duration":0.5,"rows_read":2}}"#).unwrap();
        let s = set(&raw);
        assert_eq!(s.columns, ["id", "id", "n"]);
        assert_eq!(s.rows[1][2], Val { text: "x".into(), kind: Kind::Text });
        assert_eq!(s.rows[0][2].kind, Kind::Null);
        assert_eq!(info(&[raw]), "0.5 ms · read 2");
        let q = json::parse(r#"{"results":[{"a":1,"b":"z"},{"a":2.5,"b":null}]}"#).unwrap();
        let s = set(&q);
        assert_eq!(s.columns, ["a", "b"]);
        assert_eq!(s.rows[1][0].text, "2.5");
    }
}
