// Reading changes from git: the working tree, untracked files included,
// against a base commit, split into files of diff rows; for one checkout or
// for a tree of several (a multi-repo project, or a ticket directory where
// some entries are worktrees and the rest symlinks).

use std::path::{Path, PathBuf};
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
    /// Path from the watched tree's top, so it includes the repo's directory.
    pub name: String,
    /// Index of its repo.
    pub repo: usize,
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

/// What local branch `b` was created from, as git recorded it: a ref, or for
/// `checkout -b` / `switch -c` (recorded as "HEAD") the branch HEAD moved from.
fn created_from(root: &Path, b: &str) -> Option<String> {
    let log = git(root, &["reflog", "show", "--format=%gs", &format!("refs/heads/{}", b)]).ok()?;
    let from = log.lines().last()?.strip_prefix("branch: Created from ")?;
    if from != "HEAD" {
        return Some(from.to_string());
    }
    let head = git(root, &["reflog", "show", "--format=%gs", "HEAD"]).ok()?;
    let to = format!(" to {}", b);
    head.lines().rev().filter_map(|l| l.strip_prefix("checkout: moving from ")).find_map(|l| l.strip_suffix(to.as_str()).map(String::from))
}

/// The branch the current one came off, to diff from. Follows the recorded
/// creations back through local branches (a branch made from a branch made
/// from origin/staging came off origin/staging) and stops at the first
/// remote branch; at a local branch with no record, at that branch's
/// upstream or the branch itself. With no record at all, or one that names
/// only a commit, the remote's default branch, else HEAD.
pub fn parent_ref(root: &Path) -> String {
    let resolves = |r: &str| git(root, &["rev-parse", "--verify", "-q", &format!("{}^{{commit}}", r)]).is_ok();
    let local = |r: &str| git(root, &["show-ref", "--verify", "-q", &format!("refs/heads/{}", r)]).is_ok();
    let is_sha = |r: &str| r.len() >= 7 && r.chars().all(|c| c.is_ascii_hexdigit());
    let start = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default().trim().to_string();
    let mut b = start.clone();
    let mut seen = vec![];
    while !b.is_empty() && !seen.contains(&b) && seen.len() < 16 {
        seen.push(b.clone());
        match created_from(root, &b) {
            Some(from) if !is_sha(&from) && resolves(&from) && !seen.contains(&from) => {
                if !local(&from) {
                    return from;
                }
                b = from;
            }
            _ => break,
        }
    }
    if !b.is_empty() && b != start {
        let up = git(root, &["rev-parse", "--abbrev-ref", &format!("{}@{{upstream}}", b)]).unwrap_or_default();
        let up = up.trim();
        return if up.is_empty() { b } else { up.to_string() };
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

/// A checkout being watched: its directory under the watched tree ("" when
/// the tree is the checkout) and the branch its changes are measured from.
#[derive(Clone)]
pub struct Repo {
    pub dir: String,
    pub root: PathBuf,
    pub parent: String,
}

/// Where a repo stands in a snapshot.
#[derive(Clone, PartialEq, Debug)]
pub struct Head {
    pub dir: String,
    /// The checked-out branch ("" when detached).
    pub branch: String,
    pub parent: String,
    /// The fork point from `parent`, or why there isn't one.
    pub fork: Result<String, String>,
}

/// The checkouts in `top` and one level below it (`System`, `Modules/Core`).
/// Symlinks aren't followed: in a ticket tree they point at checkouts the
/// ticket doesn't change.
pub fn find_repos(top: &Path) -> Vec<String> {
    let dirs = |p: &Path| -> Vec<(String, PathBuf)> {
        let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(p)
            .map(|rd| rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(n, _)| !n.starts_with('.')).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    let mut out = vec![];
    for (n, p) in dirs(top) {
        if p.join(".git").exists() {
            out.push(n);
        } else {
            out.extend(dirs(&p).into_iter().filter(|(_, q)| q.join(".git").exists()).map(|(m, _)| format!("{}/{}", n, m)));
        }
    }
    out
}

/// Repo `i`'s changes since its fork point, files named from the tree's top.
pub fn snapshot_repo(i: usize, r: &Repo) -> (Head, Vec<File>) {
    let branch = git(&r.root, &["symbolic-ref", "--short", "-q", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut fork = fork_point(&r.root, &r.parent);
    let mut files = Vec::new();
    if let Ok(f) = &fork {
        match snapshot(&r.root, f) {
            Ok(v) => {
                files = v
                    .into_iter()
                    .map(|mut x| {
                        if !r.dir.is_empty() {
                            x.name = format!("{}/{}", r.dir, x.name);
                        }
                        x.repo = i;
                        x
                    })
                    .collect()
            }
            Err(e) => fork = Err(e),
        }
    }
    (Head { dir: r.dir.clone(), branch, parent: r.parent.clone(), fork }, files)
}

/// Every repo's changes, in repo order.
pub fn snapshot_all(repos: &[Repo]) -> (Vec<Head>, Vec<File>) {
    let (mut heads, mut files) = (Vec::with_capacity(repos.len()), Vec::new());
    for (i, r) in repos.iter().enumerate() {
        let (h, f) = snapshot_repo(i, r);
        heads.push(h);
        files.extend(f);
    }
    (heads, files)
}

/// A checkout's git directory (for a worktree, its own one under the main repo's).
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    git(root, &["rev-parse", "--absolute-git-dir"]).ok().map(|s| PathBuf::from(s.trim()))
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
    let mut f = File { name: name.to_string(), repo: 0, status: '?', add: 0, del: 0, rows: vec![] };
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
            files.push(File { name: name.to_string(), repo: 0, status: 'M', add: 0, del: 0, rows: vec![] });
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

/// The text of `path` (from its repo's root) at commit `rev`, by lines; empty
/// when it isn't there (a file added since).
pub fn file_at(root: &Path, rev: &str, path: &str) -> Vec<String> {
    git(root, &["show", &format!("{}:{}", rev, path)]).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
}

/// A changed stretch: lines `old` of the old text became lines `new` of the new.
#[derive(Clone, PartialEq, Debug)]
pub struct Hunk {
    pub old: std::ops::Range<usize>,
    pub new: std::ops::Range<usize>,
}

/// Past this many edits, the middle is called one replaced block.
const MAX_EDITS: usize = 4000;

/// The hunks that turn `a` into `b` (Myers' algorithm, after trimming what
/// they start and end with in common).
pub fn line_hunks<S: AsRef<str>, T: AsRef<str>>(a: &[S], b: &[T]) -> Vec<Hunk> {
    let eq = |i: usize, j: usize| a[i].as_ref() == b[j].as_ref();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && eq(pre, pre) {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && eq(a.len() - 1 - suf, b.len() - 1 - suf) {
        suf += 1;
    }
    let (n, m) = (a.len() - pre - suf, b.len() - pre - suf);
    if n == 0 && m == 0 {
        return vec![];
    }
    // the matched (i, j) pairs of the middle, in order; none past MAX_EDITS
    let pairs = myers(n, m, |i, j| eq(pre + i, pre + j)).unwrap_or_default();
    let mut out: Vec<Hunk> = Vec::new();
    let mut push = |i0: usize, i1: usize, j0: usize, j1: usize| {
        if i0 < i1 || j0 < j1 {
            out.push(Hunk { old: pre + i0..pre + i1, new: pre + j0..pre + j1 });
        }
    };
    let (mut i, mut j) = (0, 0);
    for &(mi, mj) in &pairs {
        push(i, mi, j, mj);
        (i, j) = (mi + 1, mj + 1);
    }
    push(i, n, j, m);
    out
}

/// The matched pairs of a shortest edit script between sequences of `n` and
/// `m` items, or None past `MAX_EDITS` edits.
fn myers(n: usize, m: usize, eq: impl Fn(usize, usize) -> bool) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (n as isize, m as isize);
    let max = (n + m) as usize;
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // v as it was before each round, for walking back
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=max.min(MAX_EDITS) as isize {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = |k: isize| v[(off + k) as usize];
            let mut x = if k == -d || (k != d && at(k - 1) < at(k + 1)) { at(k + 1) } else { at(k - 1) + 1 };
            let mut y = x - k;
            while x < n && y < m && eq(x as usize, y as usize) {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                return Some(walk_back(&trace, n, m, &eq));
            }
            k += 2;
        }
    }
    None
}

fn walk_back(trace: &[Vec<isize>], n: isize, m: isize, eq: &impl Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        // trace[d] holds v[-d-1 ..= d+1] from before round d
        let at = |k: isize| v[(k + d + 1) as usize];
        let k = x - y;
        let (px, py) = if d == 0 {
            (0, 0)
        } else {
            let pk = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
            (at(pk), at(pk) - pk)
        };
        // back along the run of matches to just after that round's one edit
        while x > px && y > py && eq((x - 1) as usize, (y - 1) as usize) {
            pairs.push(((x - 1) as usize, (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        (x, y) = (px, py);
    }
    pairs.reverse();
    pairs
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

    /// The hunks must turn a into b, touching no more lines than needed.
    fn apply(a: &[&str], b: &[&str], edits: usize) {
        let hs = line_hunks(a, b);
        let mut out: Vec<&str> = Vec::new();
        let mut i = 0;
        for h in &hs {
            out.extend(&a[i..h.old.start]);
            out.extend(&b[h.new.clone()]);
            i = h.old.end;
        }
        out.extend(&a[i..]);
        assert_eq!(out, b, "{:?}", hs);
        assert_eq!(hs.iter().map(|h| h.old.len() + h.new.len()).sum::<usize>(), edits, "{:?}", hs);
    }

    #[test]
    fn line_diff() {
        assert_eq!(line_hunks(&["a", "b", "c"], &["a", "B", "c"]), vec![Hunk { old: 1..2, new: 1..2 }]);
        assert_eq!(line_hunks(&["a", "c"], &["a", "b", "c"]), vec![Hunk { old: 1..1, new: 1..2 }]);
        assert_eq!(line_hunks(&["a", "b", "c"], &["a", "c"]), vec![Hunk { old: 1..2, new: 1..1 }]);
        assert!(line_hunks(&["x"], &["x"]).is_empty());
        let empty: [&str; 0] = [];
        assert_eq!(line_hunks(&empty, &["a", "b"]), vec![Hunk { old: 0..0, new: 0..2 }]);
        apply(&["a", "b", "c", "d", "e", "f"], &["a", "x", "c", "e", "f", "g"], 4);
        apply(&["1", "2", "3", "4", "5", "6", "7"], &["0", "2", "3", "9", "5", "7", "8"], 6);
        apply(&["a", "b", "c", "a", "b", "b", "a"], &["c", "b", "a", "b", "a", "c"], 5);
        apply(&empty, &empty, 0);
    }

    #[test]
    fn finds_checkouts_not_symlinks() {
        let top = std::env::temp_dir().join(format!("mdiff-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        for d in ["System/.git", "Modules/Core", "Modules/plain", ".hidden/.git"] {
            std::fs::create_dir_all(top.join(d)).unwrap();
        }
        std::fs::write(top.join("Modules/Core/.git"), "gitdir: elsewhere\n").unwrap();
        std::os::unix::fs::symlink(top.join("System"), top.join("Modules/Linked")).unwrap();
        assert_eq!(find_repos(&top), vec!["Modules/Core", "System"]);
        std::fs::remove_dir_all(&top).unwrap();
    }
}
