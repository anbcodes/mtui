// Finding the token, account and database: environment, ~/.config/msql/config
// (`token …`, `account …`, `d1 NAME`), the nearest wrangler.toml / wrangler.json[c]
// (which also names the databases), and `wrangler login`'s OAuth token.

use mtui::json::{self, Value};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Found {
    pub token: Option<String>,
    pub account: Option<String>,
    /// The database asked for: an argument, `d1 …` in the config, or the only one wrangler names.
    pub db: Option<String>,
    /// Databases named by wrangler's config: (id, name).
    pub names: Vec<(String, String)>,
}

pub fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-') && s.matches('-').count() == 4
}

fn env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_default().join("msql")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))).unwrap_or_default().join("msql")
}

pub fn discover(want: Option<String>) -> Found {
    let mut f = Found { token: env(&["CLOUDFLARE_API_TOKEN", "CF_API_TOKEN"]), account: env(&["CLOUDFLARE_ACCOUNT_ID", "CF_ACCOUNT_ID"]), db: want, names: Vec::new() };
    if let Ok(s) = std::fs::read_to_string(config_dir().join("config")) {
        for l in s.lines() {
            let Some((k, v)) = l.trim().split_once(char::is_whitespace) else { continue };
            let v = v.trim().to_string();
            match k {
                "token" if f.token.is_none() => f.token = Some(v),
                "account" if f.account.is_none() => f.account = Some(v),
                "d1" | "database" if f.db.is_none() => f.db = Some(v),
                _ => {}
            }
        }
    }
    if let Some((account, dbs)) = std::env::current_dir().ok().and_then(|d| wrangler(&d)) {
        if f.account.is_none() {
            f.account = account;
        }
        if f.db.is_none() && dbs.len() == 1 {
            f.db = Some(dbs[0].0.clone());
        }
        f.names = dbs.into_iter().flat_map(|(id, name, binding)| [(id.clone(), name), (id, binding)]).filter(|(_, n)| !n.is_empty()).collect();
    }
    if f.token.is_none() {
        f.token = wrangler_token();
    }
    f
}

/// The nearest wrangler config at or above `dir`: its account id and
/// databases as (id, name, binding).
fn wrangler(dir: &Path) -> Option<(Option<String>, Vec<(String, String, String)>)> {
    let mut d = Some(dir);
    while let Some(p) = d {
        for (file, jsonc) in [("wrangler.toml", false), ("wrangler.json", true), ("wrangler.jsonc", true)] {
            if let Ok(s) = std::fs::read_to_string(p.join(file)) {
                return Some(if jsonc { parse_json(&s) } else { parse_toml(&s) });
            }
        }
        d = p.parent();
    }
    None
}

fn unquote(v: &str) -> String {
    let v = v.split('#').next().unwrap_or("").trim();
    v.trim_matches(|c| c == '"' || c == '\'').to_string()
}

pub fn parse_toml(s: &str) -> (Option<String>, Vec<(String, String, String)>) {
    let mut account = None;
    let mut dbs: Vec<(String, String, String)> = Vec::new();
    let mut in_db = false;
    for l in s.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_db = l.starts_with("[[") && l.trim_matches(|c| c == '[' || c == ']').ends_with("d1_databases");
            if in_db {
                dbs.push(Default::default());
            }
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), unquote(v));
        match (in_db, k) {
            (true, "database_id") => dbs.last_mut().unwrap().0 = v,
            (true, "database_name") => dbs.last_mut().unwrap().1 = v,
            (true, "binding") => dbs.last_mut().unwrap().2 = v,
            (false, "account_id") => account = Some(v),
            _ => {}
        }
    }
    dbs.retain(|d| is_uuid(&d.0));
    (account, dbs)
}

/// Drop `//` and `/* */` comments and trailing commas, which JSONC allows.
fn strip_jsonc(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut o = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '"' => {
                o.push('"');
                i += 1;
                while i < b.len() && b[i] != '"' {
                    if b[i] == '\\' && i + 1 < b.len() {
                        o.push(b[i]);
                        i += 1;
                    }
                    o.push(b[i]);
                    i += 1;
                }
                o.push('"');
                i += 1;
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(b.get(j), Some('}') | Some(']')) {
                    o.push(',');
                }
                i += 1;
            }
            c => {
                o.push(c);
                i += 1;
            }
        }
    }
    o
}

pub fn parse_json(s: &str) -> (Option<String>, Vec<(String, String, String)>) {
    let Ok(v) = json::parse(&strip_jsonc(s)) else { return (None, Vec::new()) };
    let dbs = v
        .get("d1_databases")
        .arr()
        .iter()
        .map(|d: &Value| (d.get("database_id").str().to_string(), d.get("database_name").str().to_string(), d.get("binding").str().to_string()))
        .filter(|d| is_uuid(&d.0))
        .collect();
    (v.get("account_id").opt_str().map(String::from), dbs)
}

/// The token `wrangler login` stores. It expires within hours, so it is a fallback.
fn wrangler_token() -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    [xdg.join(".wrangler/config/default.toml"), home.join(".wrangler/config/default.toml"), home.join("Library/Preferences/.wrangler/config/default.toml")].iter().find_map(|p| {
        std::fs::read_to_string(p).ok()?.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "oauth_token").then(|| unquote(v)).filter(|t| !t.is_empty())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b2a1c3d-1111-2222-3333-444455556666";

    #[test]
    fn wrangler_toml() {
        let (a, d) = parse_toml(&format!("name = \"x\"\naccount_id = \"abc\" # mine\n[[d1_databases]]\nbinding = \"DB\"\ndatabase_name = \"prod\"\ndatabase_id = \"{}\"\n[vars]\nfoo = 1\n", ID));
        assert_eq!(a.as_deref(), Some("abc"));
        assert_eq!(d, vec![(ID.to_string(), "prod".to_string(), "DB".to_string())]);
    }

    #[test]
    fn wrangler_jsonc() {
        let (a, d) = parse_json(&format!("{{ // c\n \"account_id\": \"abc\", /* x */\n \"d1_databases\": [ {{ \"binding\": \"DB\", \"database_name\": \"p//q\", \"database_id\": \"{}\", }}, ], }}", ID));
        assert_eq!(a.as_deref(), Some("abc"));
        assert_eq!(d, vec![(ID.to_string(), "p//q".to_string(), "DB".to_string())]);
    }
}
