// The review view's file list. The sidebar widget itself lives in
// `mtui::sidebar`; this builds its rows from a PR's changed files.

pub use mtui::sidebar::{draw, Geo as SideGeo, Row as SRow, State as SideState};

/// Rows for a list of changed files (already sorted by path): a header per
/// run of new directories, then the files.
pub fn changed_rows(files: &[(String, char, u8, String, bool)]) -> Vec<SRow> {
    let mut out = Vec::new();
    let mut prev: Vec<&str> = Vec::new();
    for (i, (path, mark, color, extra, dim)) in files.iter().enumerate() {
        let parts: Vec<&str> = path.split('/').collect();
        let dirs = &parts[..parts.len() - 1];
        let common = dirs.iter().zip(prev.iter()).take_while(|(a, b)| a == b).count();
        if common < dirs.len() {
            out.push(SRow { depth: common, text: format!("{}/", dirs[common..].join("/")), dir: true, open: true, dim: true, ..SRow::default() });
        }
        out.push(SRow { depth: dirs.len(), text: parts[parts.len() - 1].to_string(), mark: Some((*mark, *color)), extra: extra.clone(), dim: *dim, target: Some(i), ..SRow::default() });
        prev = dirs.to_vec();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_dirs() {
        let f = |p: &str| (p.to_string(), 'M', 0u8, String::new(), false);
        let rows = changed_rows(&[f("README.md"), f("src/a.rs"), f("src/b.rs"), f("src/util/x.rs"), f("tests/t.rs")]);
        let s: Vec<String> = rows.iter().map(|r| format!("{}{}{}", " ".repeat(r.depth), r.text, r.target.map_or(String::new(), |t| format!("#{}", t)))).collect();
        assert_eq!(s, vec!["README.md#0", "src/", " a.rs#1", " b.rs#2", " util/", "  x.rs#3", "tests/", " t.rs#4"]);
    }
}
