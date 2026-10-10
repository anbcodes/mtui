// A local SQLite file, through the `sqlite3` command (`-json` output). It
// makes msql usable offline and lets the whole program be tried without an
// account.

use crate::backend::{Backend, Output};
use crate::d1::set_of_rows;
use mtui::json;
use std::io::Write;
use std::process::{Command, Stdio};

pub struct Sqlite {
    pub path: String,
}

/// `-json` prints one array per statement that returns rows; split them apart.
fn arrays(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start, mut in_str, mut esc) = (Vec::new(), 0, 0, false, false);
    for (i, c) in s.char_indices() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (_, '\\') => esc = true,
                (_, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    out.push(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    out
}

impl Backend for Sqlite {
    fn title(&self) -> String {
        format!("sqlite · {}", self.path)
    }

    fn key(&self) -> String {
        let name = std::path::Path::new(&self.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        format!("sqlite-{}", name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect::<String>())
    }

    fn run(&self, sql: &str) -> Result<Output, String> {
        let start = std::time::Instant::now();
        let mut child = Command::new("sqlite3").args(["-json", "-bail", &self.path]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("sqlite3: {}", e))?;
        child.stdin.take().unwrap().write_all(sql.as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut sets = Vec::new();
        for a in arrays(&text) {
            sets.push(set_of_rows(&json::parse(a)?));
        }
        Ok(Output { sets, info: format!("{} ms", start.elapsed().as_millis()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arrays() {
        assert_eq!(arrays("[{\"a\":\"]\"}]\n[{\"b\":[1,2]}]\n"), ["[{\"a\":\"]\"}]", "[{\"b\":[1,2]}]"]);
    }
}
