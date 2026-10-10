// A database msql can talk to. A backend only has to run SQL; the schema is
// read with SQLite's own catalog queries unless it knows a better way.

use crate::schema::Schema;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Null,
    Num,
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Val {
    pub text: String,
    pub kind: Kind,
}

impl Val {
    pub fn null() -> Val {
        Val { text: "NULL".into(), kind: Kind::Null }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Val>>,
}

/// What one request returned: a result set per statement and a summary line
/// ("3 ms · read 12 · wrote 0").
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub sets: Vec<ResultSet>,
    pub info: String,
}

pub trait Backend: Send + Sync {
    /// Shown in the title bar, e.g. `d1 · my-db`.
    fn title(&self) -> String;
    /// A short file-system-safe id for the database, for its saved queries and cache.
    fn key(&self) -> String;
    fn run(&self, sql: &str) -> Result<Output, String>;

    fn schema(&self) -> Result<Schema, String> {
        let joined = "SELECT m.name AS tbl, m.type AS kind, p.name AS col, p.type AS ty, p.\"notnull\" AS nn, p.pk AS pk FROM sqlite_master m, pragma_table_info(m.name) p \
                      WHERE m.type IN ('table','view') AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND m.name NOT LIKE '\\_cf\\_%' ESCAPE '\\' \
                      ORDER BY m.name, p.cid";
        let rows = match self.run(joined) {
            Ok(o) => o.sets.into_iter().next().map(|s| s.rows).unwrap_or_default(),
            Err(_) => self.schema_by_table()?,
        };
        let t = |r: &Vec<crate::backend::Val>, i: usize| r.get(i).map(|v| v.text.clone()).unwrap_or_default();
        Ok(Schema::from_rows(rows.iter().map(|r| (String::new(), t(r, 0), t(r, 1) == "view", t(r, 2), t(r, 3), t(r, 4) == "1", t(r, 5).parse::<u32>().unwrap_or(0) > 0))))
    }

    /// The same rows as the joined query, one PRAGMA per table, for servers
    /// that don't allow table-valued pragma functions.
    fn schema_by_table(&self) -> Result<Vec<Vec<Val>>, String> {
        let names = self.run("SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND name NOT LIKE '\\_cf\\_%' ESCAPE '\\' ORDER BY name")?;
        let mut out = Vec::new();
        for r in names.sets.first().map(|s| s.rows.as_slice()).unwrap_or_default() {
            let (name, ty) = (r[0].text.clone(), r[1].text.clone());
            let info = self.run(&format!("PRAGMA table_info(\"{}\")", name.replace('"', "\"\"")))?;
            for c in info.sets.first().map(|s| s.rows.as_slice()).unwrap_or_default() {
                // cid, name, type, notnull, dflt_value, pk
                let v = |i: usize| c.get(i).cloned().unwrap_or_else(Val::null);
                out.push(vec![Val { text: name.clone(), kind: Kind::Text }, Val { text: ty.clone(), kind: Kind::Text }, v(1), v(2), v(3), v(5)]);
            }
        }
        Ok(out)
    }
}
