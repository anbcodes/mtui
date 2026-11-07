// Code references in a notes file: `path:line`, `path:from-to`, or a bare
// path, wherever they appear, as long as the file exists in the tree.

#[derive(Clone, PartialEq, Debug)]
pub struct Ref {
    pub path: String,
    /// 1-based lines; 0 = the whole file.
    pub from: usize,
    pub to: usize,
    /// The 0-based notes line it's on, and that line tidied up.
    pub at: usize,
    pub label: String,
}

impl Ref {
    pub fn text(&self) -> String {
        match (self.from, self.to) {
            (0, _) => self.path.clone(),
            (a, b) if b > a => format!("{}:{}-{}", self.path, a, b),
            (a, _) => format!("{}:{}", self.path, a),
        }
    }
}

fn path_char(c: char) -> bool {
    c.is_alphanumeric() || "_./-:@+".contains(c)
}

fn digits(s: &str) -> (usize, &str) {
    let n = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (s[..n].parse().unwrap_or(0), &s[n..])
}

/// One token as a reference, if it is one.
fn parse(tok: &str, exists: &dyn Fn(&str) -> bool) -> Option<(String, usize, usize)> {
    let tok = tok.trim_end_matches(['.', ',', ':', ';', '-']);
    let tok = tok.strip_prefix("./").unwrap_or(tok);
    if let Some((p, rest)) = tok.split_once(':') {
        let (from, rest) = digits(rest);
        if from > 0 && exists(p) {
            let to = rest.strip_prefix('-').map_or(from, |r| digits(r).0.max(from));
            return Some((p.to_string(), from, to));
        }
    }
    (tok.contains(['/', '.']) && !tok.contains(':') && exists(tok)).then(|| (tok.to_string(), 0, 0))
}

fn label(line: &str) -> String {
    let mut l = line.trim();
    for p in ["- ", "* ", "[ ] ", "[x] ", "[X] "] {
        l = l.strip_prefix(p).unwrap_or(l).trim_start();
    }
    l.replace('`', "")
}

/// Every reference in `lines`, in order, each place once.
pub fn refs(lines: &[String], exists: &dyn Fn(&str) -> bool) -> Vec<Ref> {
    let mut out: Vec<Ref> = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        for tok in line.split(|c: char| !path_char(c)).filter(|t| !t.is_empty()) {
            let Some((path, from, to)) = parse(tok, exists) else { continue };
            if !out.iter().any(|r| r.path == path && r.from == from && r.to == to) {
                out.push(Ref { path, from, to, at, label: label(line) });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn finds_references() {
        let have = |p: &str| ["api/src/auth.ts", "README.md", "web/app.tsx"].contains(&p);
        let notes = lines("## Plan\n1. [ ] Check `api/src/auth.ts:42-58` (API-032)\n- see README.md.\n- web/app.tsx:10:5, api/src/auth.ts:42-58 again\n- https://example.com/a.ts:3 and e.g. nothing.rs:4\n");
        let got: Vec<(String, usize)> = refs(&notes, &have).iter().map(|r| (r.text(), r.at)).collect();
        assert_eq!(got, vec![("api/src/auth.ts:42-58".into(), 1), ("README.md".into(), 2), ("web/app.tsx:10".into(), 3)]);
        assert_eq!(refs(&notes, &have)[0].label, "1. [ ] Check api/src/auth.ts:42-58 (API-032)");
        assert_eq!(refs(&lines("- [x] done in web/app.tsx:7"), &have)[0].label, "done in web/app.tsx:7");
    }
}
