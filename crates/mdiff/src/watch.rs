// Change notification for the watched checkouts, so git only runs for a repo
// when something in it changed: inotify on every directory of its working
// tree (symlinks and .git not followed) and on its git directory (commits,
// branch switches, staging).

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const MASK: u32 = libc::IN_CREATE | libc::IN_DELETE | libc::IN_MODIFY | libc::IN_CLOSE_WRITE | libc::IN_ATTRIB | libc::IN_MOVED_FROM | libc::IN_MOVED_TO;

pub struct Watch {
    fd: i32,
    /// watch descriptor -> repo index and directory
    dirs: HashMap<i32, (usize, PathBuf)>,
}

impl Watch {
    pub fn new() -> Result<Watch, String> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Watch { fd, dirs: HashMap::new() })
    }

    /// Watch one directory. False only when the system is out of watches; a
    /// directory that vanished meanwhile is fine.
    pub fn add(&mut self, repo: usize, dir: &Path) -> bool {
        let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else { return true };
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), MASK) };
        if wd < 0 {
            return std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOSPC);
        }
        self.dirs.insert(wd, (repo, dir.to_path_buf()));
        true
    }

    /// Watch `dir` and every directory under it but `.git`.
    pub fn add_tree(&mut self, repo: usize, dir: &Path) -> bool {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            if !self.add(repo, &d) {
                return false;
            }
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name() != ".git" {
                    stack.push(e.path());
                }
            }
        }
        true
    }

    /// Wait up to `ms` for changes and return the repos they touched.
    /// Directories created meanwhile get watched too.
    pub fn wait(&mut self, ms: i32) -> HashSet<usize> {
        let mut out = HashSet::new();
        let mut p = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, ms) } <= 0 {
            return out;
        }
        let mut buf = vec![0u8; 64 * 1024];
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            return out;
        }
        let hdr = std::mem::size_of::<libc::inotify_event>();
        let (mut i, n) = (0usize, n as usize);
        let mut new_dirs = vec![];
        while i + hdr <= n {
            let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(i) as *const libc::inotify_event) };
            let end = (i + hdr + ev.len as usize).min(n);
            let raw = &buf[i + hdr..end];
            let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
            i = end;
            if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                out.extend(self.dirs.values().map(|d| d.0));
                continue;
            }
            if ev.mask & libc::IN_IGNORED != 0 {
                self.dirs.remove(&ev.wd);
                continue;
            }
            let Some((repo, dir)) = self.dirs.get(&ev.wd) else { continue };
            out.insert(*repo);
            if ev.mask & libc::IN_ISDIR != 0 && ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 && name != b".git" {
                new_dirs.push((*repo, dir.join(OsStr::from_bytes(name))));
            }
        }
        for (r, d) in new_dirs {
            self.add_tree(r, &d);
        }
        out
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}
