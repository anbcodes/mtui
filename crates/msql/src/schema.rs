// What the completion knows about a database: tables, views and their columns.
// Cached on disk as JSON so completion works the moment msql starts.

use mtui::json;

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub notnull: bool,
    pub pk: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Empty for the default schema (SQLite, D1, Postgres' `public`).
    pub schema: String,
    pub name: String,
    pub view: bool,
    pub columns: Vec<Column>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    pub tables: Vec<Table>,
}

impl Table {
    /// `schema.name`, or just the name in the default schema.
    pub fn full_name(&self) -> String {
        if self.schema.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }
}

impl Schema {
    /// A table or view by name, ignoring case.
    pub fn find(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn to_json(&self) -> String {
        let mut o = String::from("{\"tables\":[");
        for (i, t) in self.tables.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(&format!("{{\"schema\":{},\"name\":{},\"view\":{},\"columns\":[", json::quote(&t.schema), json::quote(&t.name), t.view));
            for (j, c) in t.columns.iter().enumerate() {
                if j > 0 {
                    o.push(',');
                }
                o.push_str(&format!("{{\"name\":{},\"type\":{},\"notnull\":{},\"pk\":{}}}", json::quote(&c.name), json::quote(&c.ty), c.notnull, c.pk));
            }
            o.push_str("]}");
        }
        o.push_str("]}");
        o
    }

    pub fn from_json(s: &str) -> Option<Schema> {
        let v = json::parse(s).ok()?;
        let tables = v
            .get("tables")
            .arr()
            .iter()
            .map(|t| Table {
                schema: t.get("schema").str().to_string(),
                name: t.get("name").str().to_string(),
                view: t.get("view").bool(),
                columns: t
                    .get("columns")
                    .arr()
                    .iter()
                    .map(|c| Column { name: c.get("name").str().to_string(), ty: c.get("type").str().to_string(), notnull: c.get("notnull").bool(), pk: c.get("pk").bool() })
                    .collect(),
            })
            .filter(|t: &Table| !t.name.is_empty())
            .collect();
        Some(Schema { tables })
    }

    /// Build a schema from catalog rows, grouped by consecutive table:
    /// (schema, table, is_view, column, type, notnull, pk).
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String, bool, String, String, bool, bool)>) -> Schema {
        let mut s = Schema::default();
        for (schema, t, view, name, ty, notnull, pk) in rows {
            if s.tables.last().map_or(true, |l| l.name != t || l.schema != schema) {
                s.tables.push(Table { schema, name: t, view, columns: Vec::new() });
            }
            s.tables.last_mut().unwrap().columns.push(Column { name, ty, notnull, pk });
        }
        s
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let s = Schema::from_rows(vec![
            ("".into(), "users".into(), false, "id".into(), "INTEGER".into(), true, true),
            ("".into(), "users".into(), false, "na\"me".into(), "TEXT".into(), false, false),
            ("sales".into(), "v".into(), true, "x".into(), "".into(), false, false),
        ]);
        assert_eq!(s.tables.len(), 2);
        assert_eq!(Schema::from_json(&s.to_json()), Some(s));
    }
}
