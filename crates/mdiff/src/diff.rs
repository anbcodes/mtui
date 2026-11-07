// Reading changes from git: the working tree, untracked files included,
// against a base commit, split into files of diff rows.

use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RK {
    Hunk,
    Add,
    Del,
    Ctx,
    Note,
}

/// One diff line. `old`/`new` are 1-based line numbers (0 = not on that
/// side); a hunk header carries the hunk's starting lines.
#[derive(Clone, PartialEq, Debug)]
pub struct Row {
    pub kind: RK,
    pub old: u32,
    pub new: u32,
    pub text: String,
}

fn row(kind: RK, old: u32, new: u32, text: impl Into<String>) -> Row {
    Row { kind, old, new, text: text.into() }
}

#[derive(Clone, PartialEq, Debug)]
pub struct File {
    pub name: String,
    /// M modified, A added, D deleted, ? untracked
    pub status: char,
    pub add: usize,
    pub del: usize,
    pub rows: Vec<Row>,
}

/// Untracked files bigger than this are listed without their contents.
const MAX_UNTRACKED: u64 = 512 * 1024;
const MAX_UNTRACKED_FILES: usize = 300;

pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(["-c", "core.quotepath=off"]).args(args).output().map_err(|e| format!("git: {}", e))?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(e.lines().next().unwrap_or("git failed").to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The branch the current one came off, to diff from: the one its creation
/// was recorded against (`git branch`, `git worktree add -b`), else the
/// remote's default branch, else HEAD. A branch recorded only as a commit
/// falls through to the default branch, whose merge-base is the same commit
/// until that branch is merged in.
pub fn parent_ref(root: &Path) -> String {
    let resolves = |r: &str| git(root, &["rev-parse", "--verify", "-q", &format!("{}^{{commit}}", r)]).is_ok();
    let branch = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    let branch = branch.trim();
    if !branch.is_empty() {
        let log = git(root, &["reflog", "show", "--format=%gs", &format!("refs/heads/{}", branch)]).unwrap_or_default();
        if let Some(from) = log.lines().last().and_then(|l| l.strip_prefix("branch: Created from ")) {
            let named = from != "HEAD" && from != branch && !from.chars().all(|c| c.is_ascii_hexdigit());
            if named && resolves(from) {
                return from.to_string();
            }
        }
    }
    let def = git(root, &["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"]).unwrap_or_default();
    let def = def.trim();
    if !def.is_empty() && resolves(def) {
        return def.to_string();
    }
    "HEAD".into()
}

/// Where HEAD forked from `parent`: their merge-base.
pub fn fork_point(root: &Path, parent: &str) -> Result<String, String> {
    let s = git(root, &["merge-base", "HEAD", parent]).map_err(|e| if e == "git failed" { format!("{} has no common history with HEAD", parent) } else { e })?;
    Ok(s.trim().to_string())
}

/// Every change in the tree at `root` since commit `base`, sorted by path.
pub fn snapshot(root: &Path, base: &str) -> Result<Vec<File>, String> {
    let patch = git(root, &["diff", "--no-color", "--no-ext-diff", "--no-renames", "--src-prefix=a/", "--dst-prefix=b/", base, "--"])?;
    let mut files = parse(&patch);
    let others = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for name in others.split('\0').filter(|s| !s.is_empty()).take(MAX_UNTRACKED_FILES) {
        files.push(untracked(root, name));
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

fn untracked(root: &Path, name: &str) -> File {
    let mut f = File { name: name.to_string(), status: '?', add: 0, del: 0, rows: vec![] };
    let p = root.join(name);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if size > MAX_UNTRACKED {
        f.rows.push(row(RK::Note, 0, 0, format!("new file, {} KB: too big to show", size / 1024)));
        return f;
    }
    match std::fs::read(&p) {
        Ok(b) if b.contains(&0) => f.rows.push(row(RK::Note, 0, 0, "new binary file")),
        Ok(b) => {
            let s = String::from_utf8_lossy(&b);
            let n = s.lines().count();
            f.rows.push(row(RK::Hunk, 0, 1, format!("@@ -0,0 +1,{} @@ new file", n)));
            for (i, l) in s.lines().enumerate() {
                f.rows.push(row(RK::Add, 0, i as u32 + 1, l));
            }
            f.add = n;
        }
        Err(e) => f.rows.push(row(RK::Note, 0, 0, format!("cannot read: {}", e))),
    }
    f
}

fn parse_hunk(h: &str) -> (u32, u32) {
    let num = |s: &str| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(1);
    let old = h.strip_prefix("@@ -").map_or(1, num);
    let new = h.split_once(" +").map_or(1, |x| num(x.1));
    (old, new)
}

/// Files of a `git diff` with a/ and b/ prefixes.
pub fn parse(p: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    let (mut old, mut new, mut in_hunk) = (0, 0, false);
    for l in p.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let name = rest.rsplit_once(" b/").map_or(rest, |x| x.1);
            files.push(File { name: name.to_string(), status: 'M', add: 0, del: 0, rows: vec![] });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if l.starts_with("@@") {
            (old, new) = parse_hunk(l);
            in_hunk = true;
            f.rows.push(row(RK::Hunk, old, new, l));
        } else if !in_hunk {
            if l.starts_with("new file mode") {
                f.status = 'A';
            } else if l.starts_with("deleted file mode") {
                f.status = 'D';
            } else if l.starts_with("Binary files") {
                f.rows.push(row(RK::Note, 0, 0, "binary file changed"));
            } else if l.starts_with("old mode") {
                f.rows.push(row(RK::Note, 0, 0, l));
            }
        } else if let Some(t) = l.strip_prefix('+') {
            f.rows.push(row(RK::Add, 0, new, t));
            f.add += 1;
            new += 1;
        } else if let Some(t) = l.strip_prefix('-') {
            f.rows.push(row(RK::Del, old, 0, t));
            f.del += 1;
            old += 1;
        } else if !l.starts_with('\\') {
            f.rows.push(row(RK::Ctx, old, new, l.strip_prefix(' ').unwrap_or(l)));
            old += 1;
            new += 1;
        }
    }
    files
}

/// The line of the working file to edit for row `i`: its own new-side line,
/// else the next one below it, else the one above.
pub fn edit_line(rows: &[Row], i: usize) -> usize {
    let line = |r: &Row| if r.kind == RK::Del || r.kind == RK::Note { 0 } else { r.new };
    let below = rows.iter().skip(i).map(line).find(|&n| n > 0);
    let above = || rows.iter().take(i).rev().map(line).find(|&n| n > 0);
    below.or_else(above).unwrap_or(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@ fn main
 a
-b
+B
+c
 d
\\ No newline at end of file
diff --git a/new file.txt b/new file.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1 @@
+-- not a header
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-x
";

    #[test]
    fn files_and_counts() {
        let f = parse(PATCH);
        assert_eq!(f.iter().map(|f| (f.name.as_str(), f.status, f.add, f.del)).collect::<Vec<_>>(), vec![("src/a.rs", 'M', 2, 1), ("new file.txt", 'A', 1, 0), ("gone.rs", 'D', 0, 1)]);
        assert_eq!(f[1].rows[1].text, "-- not a header");
    }

    #[test]
    fn numbering() {
        let r = &parse(PATCH)[0].rows;
        let got: Vec<(RK, u32, u32)> = r.iter().map(|r| (r.kind, r.old, r.new)).collect();
        use RK::*;
        assert_eq!(got, vec![(Hunk, 1, 1), (Ctx, 1, 1), (Del, 2, 0), (Add, 0, 2), (Add, 0, 3), (Ctx, 3, 4)]);
    }

    #[test]
    fn lines_to_edit() {
        let r = &parse(PATCH)[0].rows;
        assert_eq!(edit_line(r, 0), 1); // hunk header: its first line
        assert_eq!(edit_line(r, 2), 2); // deleted line: where it was, the next new line
        assert_eq!(edit_line(r, 5), 4);
        let gone = &parse(PATCH)[2].rows;
        assert_eq!(edit_line(gone, 1), 1);
    }
}
