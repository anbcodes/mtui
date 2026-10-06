// Issues as the lists show them, the board's columns, and JQL helpers.

use mtui::json::Value;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub status: String,
    /// `new`, `indeterminate` or `done`: Jira's three status categories.
    pub cat: String,
    pub kind: String,
    pub priority: String,
    pub assignee: String,
    /// What the assignee is addressed by (account id on Cloud, name on Server).
    pub assignee_id: String,
    pub reporter: String,
    pub updated: String,
    pub project: String,
    pub parent: String,
    pub labels: Vec<String>,
    /// Everything the filter matches against.
    pub label: String,
}

/// A user's id as the assign calls want it.
pub fn user_id(u: &Value) -> String {
    u.get("accountId").opt_str().or(u.get("name").opt_str()).or(u.get("key").opt_str()).unwrap_or("").to_string()
}

impl Issue {
    pub fn from_json(v: &Value) -> Issue {
        let f = v.get("fields");
        let mut it = Issue {
            key: v.get("key").str().into(),
            summary: f.get("summary").str().into(),
            status: f.path("status.name").str().into(),
            cat: f.path("status.statusCategory.key").str().into(),
            kind: f.path("issuetype.name").str().into(),
            priority: f.path("priority.name").str().into(),
            assignee: f.path("assignee.displayName").str().into(),
            assignee_id: user_id(f.get("assignee")),
            reporter: f.path("reporter.displayName").str().into(),
            updated: f.get("updated").str().into(),
            project: f.path("project.key").str().into(),
            parent: f.path("parent.key").str().into(),
            labels: f.get("labels").arr().iter().map(|l| l.str().to_string()).collect(),
            label: String::new(),
        };
        it.label = format!("{} {} {} {} {} {}", it.key, it.summary, it.status, it.assignee, it.kind, it.labels.join(" "));
        it
    }
}

/// How to colour a status: category `new` is grey, `indeterminate` yellow,
/// `done` green.
pub fn cat_rank(cat: &str) -> usize {
    match cat {
        "new" => 0,
        "done" => 2,
        _ => 1,
    }
}

pub struct Column {
    pub name: String,
    pub cat: String,
    /// Indices into the issue list.
    pub items: Vec<usize>,
}

/// The board: one column per status, in workflow order (`order` is name and
/// category per status, as the project reports them), with the visible
/// issues in `vis` placed in theirs. Columns come out grouped by category.
pub fn columns(order: &[(String, String)], items: &[Issue], vis: &[usize]) -> Vec<Column> {
    let mut cols: Vec<Column> = Vec::new();
    for (n, c) in order {
        if !cols.iter().any(|x| &x.name == n) {
            cols.push(Column { name: n.clone(), cat: c.clone(), items: Vec::new() });
        }
    }
    for &i in vis {
        let it = &items[i];
        match cols.iter_mut().find(|c| c.name == it.status) {
            Some(c) => c.items.push(i),
            None => cols.push(Column { name: it.status.clone(), cat: it.cat.clone(), items: vec![i] }),
        }
    }
    cols.sort_by_key(|c| cat_rank(&c.cat));
    cols
}

/// A bare issue key like `ABC-123`.
pub fn is_key(s: &str) -> bool {
    let s = s.trim();
    match s.rsplit_once('-') {
        Some((p, n)) => !p.is_empty() && p.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// What the search prompt was given as JQL: real JQL is used as is, anything
/// else is a text search.
pub fn to_jql(s: &str) -> String {
    let s = s.trim();
    let l = format!(" {} ", s.to_lowercase());
    let jql = ["=", "~", "<", ">", " in ", " and ", " or ", " is ", " was ", "order by", " not "].iter().any(|op| l.contains(op));
    if jql {
        s.to_string()
    } else {
        format!("text ~ \"{}\" ORDER BY updated DESC", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// `1h 30m fixed it` as (`["1h", "30m"]`, `fixed it`): the time spent, then a note.
pub fn split_worklog(s: &str) -> (Vec<String>, &str) {
    let mut spent = Vec::new();
    let mut rest = s.trim();
    loop {
        let (w, r) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let unit = w.chars().last().is_some_and(|c| matches!(c, 'w' | 'd' | 'h' | 'm'));
        if w.len() < 2 || !unit || !w[..w.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            return (spent, rest);
        }
        spent.push(w.to_string());
        rest = r.trim_start();
    }
}

/// `1.2 MB`-style size.
pub fn human_size(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", n)
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtui::json;

    fn issue(key: &str, status: &str, cat: &str) -> Issue {
        Issue { key: key.into(), status: status.into(), cat: cat.into(), ..Issue::default() }
    }

    #[test]
    fn parses_issue() {
        let v = json::parse(r#"{"key":"A-1","fields":{"summary":"Fix it","status":{"name":"In Progress","statusCategory":{"key":"indeterminate"}},"issuetype":{"name":"Bug"},"assignee":{"displayName":"Ann","accountId":"u1"},"labels":["x","y"],"project":{"key":"A"},"updated":"2026-01-01T00:00:00.000+0000"}}"#).unwrap();
        let it = Issue::from_json(&v);
        assert_eq!((it.key.as_str(), it.status.as_str(), it.cat.as_str(), it.assignee_id.as_str(), it.project.as_str()), ("A-1", "In Progress", "indeterminate", "u1", "A"));
        assert!(it.label.contains("Ann") && it.label.contains("x y"));
    }

    #[test]
    fn board_columns() {
        let order = vec![("To Do".to_string(), "new".to_string()), ("In Progress".into(), "indeterminate".into()), ("Review".into(), "indeterminate".into()), ("Done".into(), "done".into())];
        let items = vec![issue("A-1", "Done", "done"), issue("A-2", "To Do", "new"), issue("A-3", "Review", "indeterminate"), issue("A-4", "Blocked", "indeterminate"), issue("A-5", "To Do", "new")];
        let cols = columns(&order, &items, &[0, 1, 2, 3, 4]);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["To Do", "In Progress", "Review", "Blocked", "Done"]);
        assert_eq!(cols[0].items, [1, 4]);
        assert!(cols[1].items.is_empty() && cols[4].items == [0]);
        let derived = columns(&[], &items, &[1, 0]);
        assert_eq!(derived.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["To Do", "Done"]);
    }

    #[test]
    fn queries() {
        assert!(is_key("ABC-123") && is_key(" a_b2-7 ") && !is_key("ABC") && !is_key("-1") && !is_key("ABC-x") && !is_key("two words-1"));
        assert_eq!(to_jql("assignee = me"), "assignee = me");
        assert_eq!(to_jql("project in (A,B) order by created"), "project in (A,B) order by created");
        assert_eq!(to_jql("login \"bug\""), "text ~ \"login \\\"bug\\\"\" ORDER BY updated DESC");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(split_worklog("1h 30m fixed it"), (vec!["1h".to_string(), "30m".to_string()], "fixed it"));
        assert_eq!(split_worklog("2d"), (vec!["2d".to_string()], ""));
        assert_eq!(split_worklog("did stuff 1h"), (vec![], "did stuff 1h"));
    }
}
