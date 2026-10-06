// Fuzzy picker (files, buffers, diagnostics, symbols, grep results).

use mtui::picker::Label;
use mtui::regex::Regex;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct PickItem {
    pub label: String,
    pub path: Option<PathBuf>,
    pub buf: Option<usize>,
    pub line: usize,
    pub col: usize,
}

impl Label for PickItem {
    fn label(&self) -> &str {
        &self.label
    }
}

pub type Picker = mtui::picker::Picker<PickItem>;

struct Ignore {
    names: Vec<String>,
    suffixes: Vec<String>,
}

impl Ignore {
    fn load(root: &Path) -> Ignore {
        let mut ig = Ignore {
            names: ["target", "node_modules", "__pycache__", "venv", ".venv", "dist", "zig-cache", "zig-out"].iter().map(|s| s.to_string()).collect(),
            suffixes: vec![".o".into(), ".pyc".into(), ".so".into(), ".a".into(), ".class".into()],
        };
        if let Ok(s) = std::fs::read_to_string(root.join(".gitignore")) {
            for l in s.lines() {
                let l = l.trim().trim_start_matches('/').trim_end_matches('/');
                if l.is_empty() || l.starts_with('#') || l.starts_with('!') {
                    continue;
                }
                if let Some(suf) = l.strip_prefix('*') {
                    if !suf.contains('*') && !suf.contains('/') {
                        ig.suffixes.push(suf.to_string());
                    }
                } else if !l.contains('*') && !l.contains('/') {
                    ig.names.push(l.to_string());
                }
            }
        }
        ig
    }
    fn skip(&self, name: &str) -> bool {
        name.starts_with('.') || self.names.iter().any(|n| n == name) || self.suffixes.iter().any(|s| name.ends_with(s.as_str()))
    }
}

pub fn walk_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let ig = Ignore::load(root);
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ig.skip(&name) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                out.push(p.strip_prefix(root).map(|p| p.to_path_buf()).unwrap_or(p));
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

pub fn grep(root: &Path, re: &Regex, limit: usize) -> Vec<PickItem> {
    let mut out = Vec::new();
    for f in walk_files(root, 20000) {
        let full = root.join(&f);
        let Ok(meta) = std::fs::metadata(&full) else { continue };
        if meta.len() > 2_000_000 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&full) else { continue };
        if bytes[..bytes.len().min(1024)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (ln, l) in text.lines().enumerate() {
            if let Some((a, _)) = re.find_at(l, 0) {
                let t: String = l.trim().chars().take(200).collect();
                out.push(PickItem { label: format!("{}:{}: {}", f.display(), ln + 1, t), path: Some(f.clone()), buf: None, line: ln, col: a });
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}
