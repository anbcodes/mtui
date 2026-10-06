// The code browser: the repo's file tree on the left, the selected file
// (highlighted, with a cursor) on the right. One request fetches the whole
// tree, so expanding folders is instant.

use super::review::{draw_rows, Content, FileDiff, Rev};
use super::side::{self, SRow, SideState};
use super::{App, FileItem, Mode, ACCENT, BG_BAR, FG_DIM};
use crate::api::{self, Req, Tag};
use mtui::json::Value;
use mtui::picker::Picker;
use mtui::screen::{Style, BOLD};
use mtui::term::{Key, Mouse, MouseKind};
use std::collections::{HashMap, HashSet};

const BIG_FILE: u64 = 2 << 20;

pub struct TNode {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub dir: bool,
    pub size: u64,
}

#[derive(Default)]
pub struct Browse {
    /// Which repo `nodes` belong to.
    pub repo: String,
    pub branch: String,
    pub nodes: Vec<TNode>,
    children: HashMap<String, Vec<usize>>,
    index: HashMap<String, usize>,
    pub loading: bool,
    pub etag: Option<String>,
    pub truncated: bool,
    pub expanded: HashSet<String>,
    /// Nodes showing in the sidebar, in order.
    pub vis: Vec<usize>,
    pub sel: usize,
    pub side_st: SideState,
    pub focus_code: bool,
    pub hide_side: bool,
    pub path: Option<String>,
    pub rev: Rev,
}

fn is_image(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".jpeg")
}

impl Browse {
    pub fn loaded(&self) -> bool {
        !self.nodes.is_empty()
    }

    /// Build the tree from a git tree listing.
    pub fn set_tree(&mut self, entries: &[Value]) {
        let mut nodes: Vec<TNode> = entries
            .iter()
            .filter(|e| matches!(e.get("type").str(), "blob" | "tree"))
            .map(|e| {
                let path = e.get("path").str().to_string();
                let name = path.rsplit('/').next().unwrap_or("").to_string();
                TNode { depth: path.matches('/').count(), dir: e.get("type").str() == "tree", size: e.get("size").num() as u64, name, path }
            })
            .collect();
        nodes.sort_by(|a, b| a.path.cmp(&b.path));
        self.index = nodes.iter().enumerate().map(|(i, n)| (n.path.clone(), i)).collect();
        self.children.clear();
        for (i, n) in nodes.iter().enumerate() {
            let parent = n.path.rsplit_once('/').map_or("", |x| x.0).to_string();
            self.children.entry(parent).or_default().push(i);
        }
        for v in self.children.values_mut() {
            v.sort_by(|&a, &b| nodes[b].dir.cmp(&nodes[a].dir).then_with(|| nodes[a].name.to_lowercase().cmp(&nodes[b].name.to_lowercase())));
        }
        self.nodes = nodes;
        self.expanded.retain(|p| self.index.contains_key(p));
        self.rebuild();
    }

    /// Recompute the visible rows; keeps the selection on the same path.
    pub fn rebuild(&mut self) {
        let keep = self.vis.get(self.sel).map(|&i| self.nodes[i].path.clone());
        let mut vis = Vec::new();
        let mut stack: Vec<usize> = self.children.get("").map(|v| v.iter().rev().copied().collect()).unwrap_or_default();
        while let Some(i) = stack.pop() {
            vis.push(i);
            let n = &self.nodes[i];
            if n.dir && self.expanded.contains(&n.path) {
                if let Some(c) = self.children.get(&n.path) {
                    stack.extend(c.iter().rev().copied());
                }
            }
        }
        self.vis = vis;
        self.sel = keep.and_then(|p| self.index.get(&p)).and_then(|i| self.vis.iter().position(|v| v == i)).unwrap_or(self.sel).min(self.vis.len().saturating_sub(1));
    }

    /// Expand every folder above `path` and select it.
    pub fn reveal(&mut self, path: &str) {
        let mut p = String::new();
        for part in path.split('/').take(path.matches('/').count()) {
            if !p.is_empty() {
                p.push('/');
            }
            p.push_str(part);
            self.expanded.insert(p.clone());
        }
        self.rebuild();
        if let Some(i) = self.index.get(path).and_then(|i| self.vis.iter().position(|v| v == i)) {
            self.sel = i;
        }
    }

    pub fn url(&self, kind: &str, path: &str) -> String {
        format!("https://github.com/{}/{}/{}/{}", self.repo, kind, self.branch, path)
    }
}

impl App {
    /// Start loading the repo's tree if it isn't (or is for another repo).
    pub(super) fn ensure_tree(&mut self, force: bool) {
        let Some(repo) = self.repo.clone() else { return };
        if self.code.loading || (!force && self.code.repo == repo && self.code.loaded()) {
            return;
        }
        if self.code.repo != repo {
            self.code = Default::default();
            self.code.repo = repo.clone();
        }
        self.code.loading = true;
        self.info("loading files…");
        self.call(Tag::RepoInfo, Req::get(format!("/repos/{}", repo)));
    }

    pub(super) fn on_repo_info(&mut self, res: Result<api::Resp, String>) {
        match res {
            Ok(r) => {
                self.code.branch = r.body.get("default_branch").opt_str().unwrap_or("HEAD").to_string();
                let mut req = Req::get(format!("/repos/{}/git/trees/{}?recursive=1", self.code.repo, mhttp::urlencode(&self.code.branch)));
                req.etag = self.code.etag.clone();
                self.call(Tag::Tree, req);
            }
            Err(e) => {
                self.code.loading = false;
                self.error(format!("repo: {}", e));
            }
        }
    }

    pub(super) fn on_tree(&mut self, res: Result<api::Resp, String>) {
        self.code.loading = false;
        let r = match res {
            Ok(r) => r,
            Err(e) => return self.error(format!("files: {}", e)),
        };
        if r.status == 304 {
            return self.info("");
        }
        self.code.etag = r.etag;
        self.code.truncated = r.body.get("truncated").bool();
        self.code.set_tree(r.body.get("tree").arr());
        self.info(if self.code.truncated { "the repo is too big to list completely" } else { "" });
        if self.code.path.is_none() {
            let readme = self.code.children.get("").and_then(|v| v.iter().copied().find(|&i| !self.code.nodes[i].dir && self.code.nodes[i].name.to_ascii_lowercase().starts_with("readme")));
            if let Some(i) = readme {
                self.open_node(i, false);
            }
        }
    }

    /// Open a file node in the viewer; `focus` moves the cursor there.
    pub(super) fn open_node(&mut self, i: usize, focus: bool) {
        let b = &mut self.code;
        let Some(n) = b.nodes.get(i) else { return };
        let (path, size) = (n.path.clone(), n.size);
        b.reveal(&path);
        b.path = Some(path.clone());
        b.focus_code = focus;
        let mut rev = Rev { single: true, full: true, ..Rev::default() };
        rev.files = Some(vec![FileDiff { name: path.clone(), status: "modified".into(), add: 0, del: 0, patch: None }]);
        rev.select_file(0);
        let image = is_image(&path);
        if !image {
            let c = if size > BIG_FILE { Content::Failed("file too large; o opens it on GitHub".into()) } else { Content::Loading };
            let load = matches!(c, Content::Loading);
            rev.contents.insert(path.clone(), c);
            b.rev = rev;
            if load {
                let enc = path.split('/').map(mhttp::urlencode).collect::<Vec<_>>().join("/");
                let req = Req { accept: api::RAW, ..Req::get(format!("/repos/{}/contents/{}?ref={}", b.repo, enc, mhttp::urlencode(&b.branch))) };
                self.call(Tag::Code(path), req);
            }
        } else {
            b.rev = rev;
        }
    }

    pub(super) fn on_code(&mut self, path: String, res: Result<api::Resp, String>) {
        if self.code.path.as_deref() != Some(path.as_str()) {
            return;
        }
        let c = match res {
            Ok(r) if r.bytes.contains(&0) => Content::Failed("binary file".into()),
            Ok(r) => Content::Text(r.text.lines().map(String::from).collect()),
            Err(e) => Content::Failed(e),
        };
        self.code.rev.contents.insert(path, c);
        self.code.rev.ver += 1;
    }

    fn code_url(&self, with_line: bool) -> Option<String> {
        let b = &self.code;
        let path = b.path.as_ref()?;
        let line = b.rev.rows.get(b.rev.cursor).filter(|r| with_line && r.kind.is_code()).map(|r| format!("#L{}", r.new)).unwrap_or_default();
        Some(format!("{}{}", b.url("blob", path), line))
    }

    pub(super) fn code_find(&mut self) {
        let items: Vec<FileItem> = self.code.nodes.iter().enumerate().filter(|(_, n)| !n.dir).map(|(i, n)| FileItem(n.path.clone(), i)).collect();
        if items.is_empty() {
            return self.error("no files loaded yet");
        }
        self.mode = Mode::Files(Picker::new("find file", items));
    }

    pub(super) fn code_key(&mut self, k: Key, g: bool) {
        let page = (self.screen.h.saturating_sub(4) / 2).max(1) as isize;
        match k {
            Key::Char('q') if !self.code.focus_code || self.code.hide_side || self.code.path.is_none() => return self.quit = true,
            Key::Char('?') => return self.mode = Mode::Help,
            Key::Char(c @ '1'..='6') => return self.set_tab(c as usize - '1' as usize),
            Key::Char('L') => return self.set_tab(self.tab + 1),
            Key::Char('H') => return self.set_tab(self.tab + super::TABS.len() - 1),
            Key::Char('R') => return self.pick_repo(),
            Key::Char('/') | Key::Ctrl('p') => return self.code_find(),
            Key::Char('r') => return self.ensure_tree(true),
            Key::Char('b') => {
                self.code.hide_side = !self.code.hide_side;
                if self.code.hide_side && self.code.path.is_some() {
                    self.code.focus_code = true;
                }
                return;
            }
            Key::Char('o') => {
                let u = if self.code.focus_code { self.code_url(true) } else { self.code.vis.get(self.code.sel).map(|&i| self.code.url(if self.code.nodes[i].dir { "tree" } else { "blob" }, &self.code.nodes[i].path.clone())) };
                if let Some(u) = u {
                    self.open_url(&u);
                }
                return;
            }
            Key::Char('y') => {
                if let Some(u) = self.code_url(true) {
                    self.screen.osc52(&u);
                    self.info(format!("copied {}", u));
                }
                return;
            }
            Key::Tab if self.code.path.is_some() => return self.code.focus_code = !self.code.focus_code,
            _ => {}
        }
        if self.code.focus_code && self.code.path.is_some() {
            let rev = &mut self.code.rev;
            match k {
                Key::Char('q') | Key::Esc => self.code.focus_code = false,
                Key::Char('j') | Key::Down | Key::Ctrl('n') | Key::Enter => rev.step(1),
                Key::Char('k') | Key::Up | Key::Ctrl('p') => rev.step(-1),
                Key::Ctrl('d') | Key::PageDown | Key::Char(' ') | Key::Ctrl('f') => rev.step(page),
                Key::Ctrl('u') | Key::PageUp | Key::Ctrl('b') => rev.step(-page),
                Key::Char('G') | Key::End => rev.goto(usize::MAX),
                Key::Home => rev.goto(0),
                Key::Char('g') if g => rev.goto(0),
                Key::Char('g') => self.pending_g = true,
                Key::Char('h') | Key::Left => rev.hscroll = rev.hscroll.saturating_sub(8),
                Key::Char('l') | Key::Right => rev.hscroll += 8,
                _ => {}
            }
            return;
        }
        let b = &mut self.code;
        let n = b.vis.len();
        let mv = |b: &mut Browse, d: isize| b.sel = (b.sel as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
        match k {
            Key::Char('j') | Key::Down | Key::Ctrl('n') => mv(b, 1),
            Key::Char('k') | Key::Up | Key::Ctrl('p') => mv(b, -1),
            Key::Ctrl('d') | Key::PageDown => mv(b, page),
            Key::Ctrl('u') | Key::PageUp => mv(b, -page),
            Key::Char('G') | Key::End => mv(b, isize::MAX / 2),
            Key::Home => mv(b, isize::MIN / 2),
            Key::Char('g') if g => mv(b, isize::MIN / 2),
            Key::Char('g') => self.pending_g = true,
            Key::Enter | Key::Char('l') | Key::Right => self.code_activate(),
            Key::Char('h') | Key::Left | Key::Backspace => {
                let Some(&i) = b.vis.get(b.sel) else { return };
                let node = &b.nodes[i];
                if node.dir && b.expanded.contains(&node.path) {
                    let p = node.path.clone();
                    b.expanded.remove(&p);
                    b.rebuild();
                } else if let Some((parent, _)) = node.path.rsplit_once('/') {
                    if let Some(p) = b.index.get(parent).and_then(|i| b.vis.iter().position(|v| v == i)) {
                        b.sel = p;
                    }
                }
            }
            _ => {}
        }
    }

    /// Enter on the selected row: toggle a folder or open a file.
    fn code_activate(&mut self) {
        let b = &mut self.code;
        let Some(&i) = b.vis.get(b.sel) else { return };
        if b.nodes[i].dir {
            let p = b.nodes[i].path.clone();
            if !b.expanded.remove(&p) {
                b.expanded.insert(p);
            }
            b.rebuild();
        } else {
            self.open_node(i, true);
        }
    }

    pub(super) fn code_mouse(&mut self, m: Mouse) {
        let side = self.side;
        let on_side = side.w > 0 && m.x <= side.w;
        let b = &mut self.code;
        match m.kind {
            MouseKind::WheelUp if on_side => b.side_st.top = b.side_st.top.saturating_sub(3),
            MouseKind::WheelDown if on_side => b.side_st.top += 3,
            MouseKind::WheelUp => b.rev.scroll_view(-3),
            MouseKind::WheelDown => b.rev.scroll_view(3),
            MouseKind::Press(0) if on_side && m.y >= side.y0 => {
                if let Some(Some(vi)) = self.side_map.get(side.top + m.y - side.y0).copied() {
                    b.sel = vi;
                    b.focus_code = false;
                    self.code_activate();
                }
            }
            MouseKind::Press(0) if !on_side && m.y >= 2 => {
                let i = b.rev.top + m.y - 2;
                if b.rev.rows.get(i).is_some_and(|r| r.kind.selectable()) {
                    b.rev.cursor = i;
                    b.focus_code = true;
                    b.rev.remember();
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_code(&mut self, bottom: usize) {
        let w = self.screen.w;
        self.draw_tabs();
        let h = bottom.saturating_sub(2);
        let b = &mut self.code;
        self.side = Default::default();
        let hdr = Style::new(252, BG_BAR, 0);
        self.screen.fill(0, w, 1, hdr);
        if self.repo.is_none() {
            self.screen.set_images(Vec::new());
            self.screen.puts(2, 3, "no repo; press C-k to pick one", Style::new(FG_DIM, 0, mtui::screen::ITALIC), w);
            return;
        }
        if !b.loaded() {
            self.screen.set_images(Vec::new());
            self.screen.puts(2, 3, if b.loading { "loading…" } else { "nothing to show" }, Style::new(FG_DIM, 0, mtui::screen::ITALIC), w);
            return;
        }
        let narrow = w < 60;
        let (side_w, show_code) = if narrow {
            if b.focus_code { (0, true) } else { (w - 1, false) }
        } else if b.hide_side {
            (0, true)
        } else {
            ((w / 4).clamp(24, 40), true)
        };
        let x0 = if side_w > 0 && show_code { side_w + 1 } else { 0 };
        if side_w > 0 {
            let rows: Vec<SRow> = b
                .vis
                .iter()
                .enumerate()
                .map(|(vi, &i)| {
                    let n = &b.nodes[i];
                    SRow { depth: n.depth, text: if n.dir { format!("{}/", n.name) } else { n.name.clone() }, dir: n.dir, open: b.expanded.contains(&n.path), mark: None, extra: String::new(), dim: false, target: Some(vi) }
                })
                .collect();
            let title = format!(" {}{}", b.repo, if b.truncated { " (partial)" } else { "" });
            self.screen.puts(0, 1, &title, Style::new(ACCENT, BG_BAR, BOLD), side_w);
            self.side_map = rows.iter().map(|r| r.target).collect();
            self.side = side::draw(&mut self.screen, &rows, Some(b.sel), &mut b.side_st, (0, side_w), (2, h), !b.focus_code);
        }
        if !show_code {
            self.screen.set_images(Vec::new());
            return;
        }
        let Some(path) = b.path.clone() else {
            self.screen.set_images(Vec::new());
            self.screen.puts(x0 + 2, 3, "select a file", Style::new(FG_DIM, 0, mtui::screen::ITALIC), w);
            return;
        };
        let title = format!(" {}  [{}]", path, b.branch);
        self.screen.puts(x0, 1, &title, Style::new(if b.focus_code { 255 } else { 250 }, BG_BAR, BOLD), w);
        let (ims, needs) = draw_rows(&mut self.screen, &mut self.gallery, &mut b.rev, &[], &b.branch.clone(), (x0, w - x0), (2, h));
        let (repo, branch) = (b.repo.clone(), b.branch.clone());
        let overlay = matches!(self.mode, Mode::Pick(_) | Mode::Files(_) | Mode::Help);
        self.screen.set_images(if overlay { Vec::new() } else { ims });
        self.fetch_blobs(&repo, &branch, needs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtui::json;

    fn tree() -> Browse {
        let v = json::parse(r#"[{"path":"src","type":"tree"},{"path":"src/main.rs","type":"blob","size":10},{"path":"src/util","type":"tree"},{"path":"src/util/a.rs","type":"blob"},{"path":"README.md","type":"blob"},{"path":"Cargo.toml","type":"blob"},{"path":"docs","type":"tree"},{"path":"sub","type":"commit"}]"#).unwrap();
        let mut b = Browse::default();
        b.set_tree(v.arr());
        b
    }

    fn names(b: &Browse) -> Vec<String> {
        b.vis.iter().map(|&i| format!("{}{}", " ".repeat(b.nodes[i].depth), b.nodes[i].path.rsplit('/').next().unwrap())).collect()
    }

    #[test]
    fn dirs_first_collapsed() {
        let b = tree();
        assert_eq!(names(&b), vec!["docs", "src", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn reveal_expands_parents() {
        let mut b = tree();
        b.reveal("src/util/a.rs");
        assert_eq!(names(&b), vec!["docs", "src", " util", "  a.rs", " main.rs", "Cargo.toml", "README.md"]);
        assert_eq!(b.nodes[b.vis[b.sel]].path, "src/util/a.rs");
        // collapsing keeps the selection on the same path or clamps
        b.expanded.remove("src");
        b.rebuild();
        assert!(b.sel < b.vis.len());
    }
}
