// Command-line mode (: / ?) and ex commands.

use crate::buffer::Buffer;
use crate::editor::{Editor, Mode};
use crate::picker::Picker;
use mtui::regex::Regex;
use mtui::term::Key;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const HELP: &str = "\
mvi — minimal vim-like editor                       (:bd closes this)
NORMAL  h j k l w b e W B E ge 0 ^ $ gg G { } % f F t T ; , H M L  counts work
        i a I A o O  x X D C s S r~ J gJ  p P  u ^R  .  ~  ^A ^X  * # n N
        d c y > < gc(comment) gu gU g~ + motion/text-object (iw aw i( a{ i\" ip ...)
        v V visual · q{r} record · @{r} play · m{a-z} mark · '{a-z} jump · '' back
        gd goto definition · gf open file · K line diagnostics · ]d [d next/prev diag
        ]e [e next/prev error · ]q [q quickfix · ]b [b buffers · ^^ alt buffer · ^S save
LEADER  <space>f files  <space>b buffers  <space>s symbols  <space>d diagnostics
        <space>/ grep  <space>c run checker  <space>y yank to system clipboard (OSC52)
INSERT  Tab/^N/^P cycle completions, Enter accepts, ^Space force, ^W ^U ^R{reg} ^T ^D
EX      :w :q :wq :x :q! :qa :wa :e file :e! :enew :bn :bp :bd :b N :ls :N (goto)
        :[range]s/re/rep/gi  :g/re/cmd  :v/re/cmd  :[range]!cmd (filter)  :!cmd  :r file
        :[range]d :[range]y :[range]> :[range]< :[range]sort[!u] :[range]norm keys
        :check :diag :cn :cp :grep re :files :cd :pwd :reg :noh :help
        :preview (:md) render the buffer as markdown: j k C-d C-u g G, Enter goes to the source line
        :set nu rnu ts=N sw=N et noet list ac(autocheck) acp(autocomplete) hls mouse ft=lang
        :checker <lang> <cmd>   (use {file}; prefix 'Marker|' to run where Marker lives)
MOUSE   click moves the cursor · drag selects (visual) · double-click word · triple-click line
        wheel scrolls · shift+drag uses the terminal's own selection · :set nomouse
REGEX   PCRE-style: . [] \\w \\d \\s \\b ^ $ * + ? {n,m} (a|b) captures \\1 in replacements;
        smartcase (lowercase pattern = case-insensitive)
CONFIG  ~/.config/mvi/config — one ex command per line (e.g. 'set rnu')";

impl Editor {
    pub fn cmd_key(&mut self, k: Key) {
        let Mode::Cmd(kind) = self.mode else { return };
        match k {
            Key::Esc | Key::Ctrl('c') => {
                self.cancel_cmd(kind);
                return;
            }
            Key::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                self.mode = Mode::Normal;
                if !line.is_empty() {
                    let h = format!("{}{}", kind, line);
                    self.hist.retain(|x| *x != h);
                    self.hist.push(h);
                }
                if kind == ':' {
                    let c = self.cursor();
                    self.b().mark_start(c);
                    self.ex(&line);
                    if self.mode == Mode::Normal {
                        let c = self.cursor();
                        self.b().commit(c);
                        self.clamp();
                    }
                } else {
                    let (origin, top) = self.search_origin;
                    self.set_cursor(origin);
                    self.b().top = top;
                    let pat = if line.is_empty() { self.search_pat.clone() } else { line };
                    if pat.is_empty() {
                        return;
                    }
                    if self.set_search(&pat, kind == '/') {
                        let re = self.search.take().unwrap();
                        match self.search_from(origin, kind == '/', &re) {
                            Some((p, _)) => {
                                self.last_jump = origin;
                                self.set_cursor(p);
                                self.clamp();
                            }
                            None => self.err(format!("pattern not found: {}", pat)),
                        }
                        self.search = Some(re);
                    }
                }
                return;
            }
            Key::Backspace => {
                if self.cmdline.pop().is_none() {
                    self.cancel_cmd(kind);
                    return;
                }
            }
            Key::Ctrl('u') => self.cmdline.clear(),
            Key::Ctrl('w') => {
                let t = self.cmdline.trim_end().len();
                self.cmdline.truncate(t);
                while let Some(c) = self.cmdline.chars().last() {
                    if c.is_alphanumeric() || c == '_' {
                        self.cmdline.pop();
                    } else {
                        break;
                    }
                }
            }
            Key::Up | Key::Down => {
                let matches: Vec<usize> = self.hist.iter().enumerate().filter(|(_, h)| h.starts_with(kind)).map(|(i, _)| i).collect();
                if matches.is_empty() {
                    return;
                }
                let pos = matches.iter().position(|&i| i >= self.hist_idx).unwrap_or(matches.len());
                let np = if k == Key::Up { pos.saturating_sub(1) } else { pos + 1 };
                if np >= matches.len() {
                    self.hist_idx = self.hist.len();
                    self.cmdline.clear();
                } else {
                    self.hist_idx = matches[np];
                    self.cmdline = self.hist[self.hist_idx][1..].to_string();
                }
            }
            Key::Tab if kind == ':' => self.cmd_complete(),
            Key::Ctrl('r') => {
                // ^R inserts the word under the cursor
                if let Some(w) = self.word_under_cursor() {
                    self.cmdline.push_str(&w);
                }
            }
            Key::Char(c) => self.cmdline.push(c),
            Key::Paste(s) => self.cmdline.push_str(s.lines().next().unwrap_or("")),
            _ => {}
        }
        if kind != ':' {
            self.incsearch(kind == '/');
        }
    }

    fn cancel_cmd(&mut self, kind: char) {
        self.mode = Mode::Normal;
        self.cmdline.clear();
        if kind != ':' {
            let (origin, top) = self.search_origin;
            self.set_cursor(origin);
            self.b().top = top;
            // drop the incremental pattern, restore the previous search
            self.search = if self.search_pat.is_empty() { None } else { Regex::new(&self.search_pat, true).ok() };
            self.show_hl = false;
        }
    }

    fn incsearch(&mut self, fwd: bool) {
        let (origin, top) = self.search_origin;
        self.set_cursor(origin);
        self.b().top = top;
        if self.cmdline.is_empty() {
            return;
        }
        if let Ok(re) = Regex::new(&self.cmdline, true) {
            if let Some((p, _)) = self.search_from(origin, fwd, &re) {
                self.set_cursor(p);
            }
            self.search = Some(re);
            self.show_hl = true;
        }
    }

    fn cmd_complete(&mut self) {
        let Some(sp) = self.cmdline.rfind(' ') else {
            // complete command names
            let cmds = ["write", "quit", "edit", "enew", "bnext", "bprev", "bdelete", "buffers", "set", "check", "checker", "diag", "grep", "files", "help", "nohlsearch", "preview", "registers", "sort", "normal", "global", "substitute", "read", "pwd", "cd", "wall", "qall", "xit"];
            let m: Vec<&str> = cmds.iter().copied().filter(|c| c.starts_with(self.cmdline.as_str())).collect();
            if m.len() == 1 {
                self.cmdline = m[0].to_string() + " ";
            } else if !m.is_empty() {
                self.info(m.join("  "));
            }
            return;
        };
        let word = self.cmdline[sp + 1..].to_string();
        let head = self.cmdline[..sp].trim();
        if head == "set" || head == "se" {
            return;
        }
        let w2 = if word.contains('/') { word.clone() } else { format!("./{}", word) };
        let line = format!("{}", w2);
        let mut tmp = Buffer::new(None);
        tmp.lines = vec![line.clone()];
        let syms = Default::default();
        let Some(c) = crate::complete::complete(&tmp, 0, line.len(), true, &syms) else { return };
        let strip = |s: &str| if word.contains('/') { s.to_string() } else { s.trim_start_matches("./").to_string() };
        let items: Vec<String> = c.items.iter().map(|i| strip(&i.text)).collect();
        if items.len() == 1 {
            self.cmdline = format!("{} {}", &self.cmdline[..sp], items[0]);
        } else if !items.is_empty() {
            // longest common prefix
            let mut lcp = items[0].clone();
            for it in &items[1..] {
                while !it.starts_with(&lcp) {
                    lcp.pop();
                }
            }
            if lcp.len() > word.len() {
                self.cmdline = format!("{} {}", &self.cmdline[..sp], lcp);
            }
            let shown: Vec<&str> = items.iter().take(30).map(|s| s.rsplit('/').find(|x| !x.is_empty()).unwrap_or(s)).collect();
            self.info(shown.join("  "));
        }
    }

    fn parse_addr(&self, s: &[u8], i: &mut usize) -> Option<usize> {
        let nl = self.bb().lines.len();
        let cy = self.cursor().0;
        let mut base: Option<isize> = None;
        if *i < s.len() {
            match s[*i] {
                b'0'..=b'9' => {
                    let st = *i;
                    while *i < s.len() && s[*i].is_ascii_digit() {
                        *i += 1;
                    }
                    let n: isize = std::str::from_utf8(&s[st..*i]).unwrap().parse().unwrap_or(1);
                    base = Some(n - 1);
                }
                b'.' => {
                    *i += 1;
                    base = Some(cy as isize);
                }
                b'$' => {
                    *i += 1;
                    base = Some(nl as isize - 1);
                }
                b'\'' if *i + 1 < s.len() => {
                    let m = s[*i + 1] as char;
                    *i += 2;
                    let p = match m {
                        '<' => self.last_vis.map(|v| v.0.min(v.1).0),
                        '>' => self.last_vis.map(|v| v.0.max(v.1).0),
                        'a'..='z' => self.bb().marks[m as usize - 'a' as usize].map(|p| p.0),
                        _ => None,
                    }?;
                    base = Some(p as isize);
                }
                _ => {}
            }
        }
        while *i < s.len() && (s[*i] == b'+' || s[*i] == b'-') {
            let neg = s[*i] == b'-';
            *i += 1;
            let st = *i;
            while *i < s.len() && s[*i].is_ascii_digit() {
                *i += 1;
            }
            let n: isize = std::str::from_utf8(&s[st..*i]).unwrap().parse().unwrap_or(1);
            base = Some(base.unwrap_or(cy as isize) + if neg { -n } else { n });
        }
        base.map(|b| b.clamp(0, nl as isize - 1) as usize)
    }

    pub fn ex(&mut self, cmd: &str) {
        let cmd = cmd.trim_start_matches([' ', ':']).trim_end();
        if cmd.is_empty() {
            return;
        }
        let s = cmd.as_bytes();
        let mut i = 0;
        let cy = self.cursor().0;
        let nl = self.bb().lines.len();
        let mut range: Option<(usize, usize)> = None;
        if s[0] == b'%' {
            range = Some((0, nl - 1));
            i = 1;
        } else if let Some(a) = self.parse_addr(s, &mut i) {
            let mut b = a;
            if i < s.len() && (s[i] == b',' || s[i] == b';') {
                i += 1;
                b = self.parse_addr(s, &mut i).unwrap_or(a);
            }
            range = Some((a.min(b), a.max(b)));
        } else if s[0] == b'\'' {
            self.err("mark not set");
            return;
        }
        let rest = &cmd[i..].trim_start();
        let name_len = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let (name, mut args) = if name_len == 0 && !rest.is_empty() {
            let c = rest.chars().next().unwrap();
            (&rest[..c.len_utf8()], &rest[c.len_utf8()..])
        } else {
            (&rest[..name_len], &rest[name_len..])
        };
        // :s and :g take a delimiter immediately; :normal takes raw keys
        let special = matches!(name, "s" | "substitute" | "g" | "global" | "v" | "vglobal" | "norm" | "normal");
        let bang = !special && args.starts_with('!');
        if bang {
            args = &args[1..];
        }
        let args = if special { args.strip_prefix(' ').unwrap_or(args) } else { args.trim() };
        let (r1, r2) = range.unwrap_or((cy, cy));
        match name {
            "" => {
                if let Some((_, b)) = range {
                    self.last_jump = self.cursor();
                    let x = self.bb().lines[b].len() - self.bb().lines[b].trim_start().len();
                    self.set_cursor((b, x));
                }
            }
            "w" | "write" | "up" | "update" => {
                if name.starts_with('u') && !self.bb().dirty {
                    return;
                }
                self.write(args, range);
            }
            "wq" | "x" | "xit" | "exit" => {
                if name == "wq" || self.bb().dirty {
                    if !self.write(args, None) {
                        return;
                    }
                }
                self.quit_all(bang);
            }
            "q" | "quit" | "qa" | "qall" | "quita" | "quitall" | "cq" => self.quit_all(bang),
            "wa" | "wall" | "wqa" | "wqall" | "xa" | "xall" => {
                let mut failed = false;
                for k in 0..self.bufs.len() {
                    if self.bufs[k].dirty && self.bufs[k].path.is_some() {
                        if let Err(e) = self.bufs[k].save() {
                            self.err(format!("{}: {}", self.bufs[k].name, e));
                            failed = true;
                        }
                    }
                }
                if !failed {
                    self.info("all buffers written");
                    if name != "wa" && name != "wall" {
                        self.quit_all(false);
                    }
                }
            }
            "e" | "edit" => {
                if args.is_empty() {
                    if self.bb().dirty && !bang {
                        self.err("buffer modified (add ! to discard)");
                        return;
                    }
                    let Some(p) = self.bb().path.clone() else { return };
                    let (cy, cx) = self.cursor();
                    let top = self.bb().top;
                    self.bufs[self.cur] = Buffer::new(Some(p));
                    self.b().top = top;
                    self.set_cursor((cy, cx));
                    self.clamp();
                    self.info("reloaded");
                    self.screen.invalidate();
                } else {
                    let p = expand_home(args);
                    self.open_file(&p);
                }
            }
            "enew" | "ene" => {
                self.bufs.push(Buffer::new(None));
                let n = self.bufs.len() - 1;
                self.switch_buf(n);
            }
            "bn" | "bnext" => {
                let i = (self.cur + 1) % self.bufs.len();
                self.switch_buf(i);
            }
            "bp" | "bprev" | "bprevious" | "bN" => {
                let n = self.bufs.len();
                self.switch_buf((self.cur + n - 1) % n);
            }
            "bd" | "bdelete" | "bw" | "bwipe" => {
                if self.bb().dirty && !bang {
                    self.err("buffer modified (add ! to discard)");
                    return;
                }
                let c = self.cur;
                self.close_buf(c);
            }
            "b" | "buffer" => {
                if let Ok(n) = args.parse::<usize>() {
                    if n >= 1 && n <= self.bufs.len() {
                        self.switch_buf(n - 1);
                    } else {
                        self.err("no such buffer");
                    }
                } else if !args.is_empty() {
                    match self.bufs.iter().position(|b| b.name.contains(args)) {
                        Some(i) => self.switch_buf(i),
                        None => self.err("no matching buffer"),
                    }
                } else {
                    self.open_buffer_picker();
                }
            }
            "ls" | "buffers" | "files" | "find" | "fin" if name.starts_with('l') || name.starts_with('b') => self.open_buffer_picker(),
            "files" | "find" | "fin" => {
                self.open_files_picker();
                if let Some(p) = &mut self.picker {
                    p.query = args.to_string();
                    p.refilter();
                }
            }
            "s" | "substitute" | "&" => self.substitute(r1, r2, if name == "&" { "" } else { args }),
            "g" | "global" | "v" | "vglobal" => self.global(range.unwrap_or((0, nl - 1)), args, name.starts_with('v') || args.starts_with('!')),
            "noh" | "nohlsearch" => self.show_hl = false,
            "set" | "se" => self.set(args),
            "check" | "make" | "mak" | "lint" => self.run_check(true),
            "checker" => {
                let mut it = args.splitn(2, char::is_whitespace);
                let (Some(lang), Some(c)) = (it.next(), it.next()) else {
                    self.err("usage: checker <lang> <command with {file}>");
                    return;
                };
                let c = c.trim().trim_start_matches('=').trim();
                self.opts.checks.entry(lang.to_string()).or_default().insert(0, c.to_string());
            }
            "diag" | "cl" | "clist" | "copen" | "cope" | "diagnostics" => self.open_diag_picker(),
            "cn" | "cnext" => self.handle_keys("]q"),
            "cp" | "cprev" | "cprevious" | "cN" => self.handle_keys("[q"),
            "grep" | "rg" | "vimgrep" | "vim" => {
                if args.is_empty() {
                    return;
                }
                let re = match Regex::new(args, true) {
                    Ok(r) => r,
                    Err(e) => return self.err(format!("bad pattern: {}", e)),
                };
                let root = std::env::current_dir().unwrap_or_default();
                let items = crate::picker::grep(&root, &re, 5000);
                if items.is_empty() {
                    self.err(format!("no matches: {}", args));
                } else {
                    self.set_search(args, true);
                    self.picker = Some(Picker::new(&format!("grep {} ({})", args, items.len()), items));
                }
            }
            "!" => {
                if range.is_some() {
                    self.filter(r1, r2, args);
                } else {
                    let out = run_shell(args, None);
                    let mut lines: Vec<String> = out.lines().map(|s| s.to_string()).collect();
                    if lines.is_empty() {
                        lines.push("(no output)".into());
                    }
                    lines.insert(0, format!(":!{}", args));
                    self.msg_lines = lines;
                }
            }
            "r" | "read" => {
                let text = if let Some(c) = args.strip_prefix('!') {
                    run_shell(c, None)
                } else {
                    match std::fs::read_to_string(expand_home(args)) {
                        Ok(t) => t,
                        Err(e) => return self.err(e.to_string()),
                    }
                };
                let text = text.strip_suffix('\n').unwrap_or(&text).to_string();
                let at = r2;
                let len = self.bb().line_len(at);
                self.b().insert((at, len), &format!("\n{}", text));
                self.set_cursor((at + 1, 0));
            }
            "d" | "delete" => {
                let reg = args.chars().next().filter(|c| c.is_ascii_alphabetic()).unwrap_or('"');
                let t = self.delete_lines(r1, r2);
                self.set_reg(reg, t, true, false);
                self.set_cursor((r1, 0));
                self.clamp();
            }
            "y" | "yank" => {
                let reg = args.chars().next().filter(|c| c.is_ascii_alphabetic() || *c == '+').unwrap_or('"');
                let t = self.bb().lines[r1..=r2].join("\n") + "\n";
                self.set_reg(reg, t, true, true);
                self.info(format!("{} lines yanked", r2 - r1 + 1));
            }
            ">" | "<" => {
                let n = 1 + args.chars().filter(|&c| c == '>' || c == '<').count();
                self.indent_lines(r1, r2, name == "<", n);
            }
            "j" | "join" => {
                self.set_cursor((r1, 0));
                let n = (r2 - r1 + 1).max(2);
                self.handle_keys(&format!("{}J", n));
            }
            "m" | "move" | "t" | "co" | "copy" => {
                let mut j = 0;
                let to = if args == "0" { None } else { self.parse_addr(args.as_bytes(), &mut j) };
                let text = self.bb().lines[r1..=r2].join("\n");
                let to_i = to.map_or(-1, |t| t as isize);
                if name.starts_with('m') && to_i >= r1 as isize - 1 && to_i <= r2 as isize {
                    return;
                }
                if name.starts_with('m') {
                    self.delete_lines(r1, r2);
                }
                let mut at = to_i;
                if name.starts_with('m') && at > r2 as isize {
                    at -= (r2 - r1 + 1) as isize;
                }
                if at < 0 {
                    self.b().insert((0, 0), &format!("{}\n", text));
                    self.set_cursor((r2 - r1, 0));
                } else {
                    let at = at as usize;
                    let len = self.bb().line_len(at);
                    self.b().insert((at, len), &format!("\n{}", text));
                    self.set_cursor((at + 1 + r2 - r1, 0));
                }
            }
            "norm" | "normal" => {
                let keys = args.to_string();
                for ln in r1..=r2 {
                    if ln >= self.bb().lines.len() {
                        break;
                    }
                    self.set_cursor((ln, 0));
                    self.mode = Mode::Normal;
                    self.handle_keys(&keys);
                    if self.mode != Mode::Normal {
                        self.handle_key(Key::Esc);
                    }
                }
            }
            "sor" | "sort" => {
                let (r1, r2) = if range.is_none() { (0, nl - 1) } else { (r1, r2) };
                let mut v: Vec<String> = self.bb().lines[r1..=r2].to_vec();
                if args.contains('n') {
                    v.sort_by_key(|l| l.trim().split(|c: char| !c.is_ascii_digit() && c != '-').find(|s| !s.is_empty()).and_then(|s| s.parse::<i64>().ok()).unwrap_or(i64::MIN));
                } else if args.contains('i') {
                    v.sort_by_key(|a| a.to_lowercase());
                } else {
                    v.sort();
                }
                if bang {
                    v.reverse();
                }
                if args.contains('u') {
                    v.dedup();
                }
                let e = self.bb().line_len(r2);
                self.b().delete((r1, 0), (r2, e));
                self.b().insert((r1, 0), &v.join("\n"));
                self.set_cursor((r1, 0));
            }
            "cd" => {
                let p = if args.is_empty() { expand_home("~") } else { expand_home(args) };
                match std::env::set_current_dir(&p) {
                    Ok(_) => self.info(format!("{}", std::env::current_dir().unwrap_or_default().display())),
                    Err(e) => self.err(e.to_string()),
                }
            }
            "pwd" => self.info(format!("{}", std::env::current_dir().unwrap_or_default().display())),
            "reg" | "registers" | "di" | "display" => {
                let mut keys: Vec<&char> = self.regs.keys().collect();
                keys.sort();
                let lines = keys
                    .into_iter()
                    .map(|k| {
                        let r = &self.regs[k];
                        let t: String = r.text.chars().take(120).map(|c| if c == '\n' { '⏎' } else { c }).collect();
                        format!("\"{}  {}{}", k, if r.line { "L " } else { "  " }, t)
                    })
                    .collect::<Vec<_>>();
                self.msg_lines = if lines.is_empty() { vec!["(no registers)".into()] } else { lines };
            }
            "md" | "preview" => self.open_preview(),
            "h" | "help" => match self.bufs.iter().position(|b| b.name == "[help]") {
                Some(i) => self.switch_buf(i),
                None => {
                    let mut b = Buffer::new(None);
                    b.lines = HELP.lines().map(|s| s.to_string()).collect();
                    b.name = "[help]".into();
                    self.bufs.push(b);
                    let i = self.bufs.len() - 1;
                    self.switch_buf(i);
                }
            },
            "ft" | "filetype" | "syntax" | "syn" => self.set(&format!("ft={}", args)),
            _ => self.err(format!("unknown command: {}", name)),
        }
    }

    pub fn handle_keys(&mut self, keys: &str) {
        let mut it = keys.chars().peekable();
        while let Some(c) = it.next() {
            // allow <Esc> <CR> <Tab> notations in :normal and macros-from-text
            if c == '<' {
                let rest: String = it.clone().take(5).collect();
                let lower = rest.to_ascii_lowercase();
                let named = [("esc>", Key::Esc), ("cr>", Key::Enter), ("tab>", Key::Tab), ("bs>", Key::Backspace)];
                if let Some((n, k)) = named.iter().find(|(n, _)| lower.starts_with(n)) {
                    for _ in 0..n.len() {
                        it.next();
                    }
                    self.handle_key(k.clone());
                    continue;
                }
            }
            let k = match c {
                '\n' | '\r' => Key::Enter,
                '\t' => Key::Tab,
                '\x1b' => Key::Esc,
                c => Key::Char(c),
            };
            self.handle_key(k);
        }
    }

    fn quit_all(&mut self, force: bool) {
        if !force {
            if let Some(b) = self.bufs.iter().find(|b| b.dirty) {
                let n = b.name.clone();
                self.err(format!("\"{}\" has unsaved changes (add ! to override, :wa to save all)", n));
                return;
            }
        }
        self.quit = true;
    }

    fn write(&mut self, args: &str, range: Option<(usize, usize)>) -> bool {
        if !args.is_empty() {
            let p = expand_home(args);
            if self.bb().path.is_none() {
                let b = self.b();
                b.name = p.display().to_string();
                b.lang = crate::syntax::detect(&p, b.lines.first().map(|s| s.as_str()).unwrap_or(""));
                b.hl = Default::default();
                b.path = Some(p);
            } else {
                // write a copy (or a range) elsewhere
                let b = self.bb();
                let (a, z) = range.unwrap_or((0, b.lines.len() - 1));
                let data = b.lines[a..=z].join("\n") + "\n";
                return match std::fs::write(&p, data) {
                    Ok(_) => {
                        self.info(format!("\"{}\" {}L written", p.display(), z - a + 1));
                        true
                    }
                    Err(e) => {
                        self.err(format!("{}: {}", p.display(), e));
                        false
                    }
                };
            }
        }
        match self.b().save() {
            Ok(n) => {
                let b = self.bb();
                let m = format!("\"{}\" {}L {}B written", b.name, b.lines.len(), n);
                self.info(m);
                if self.opts.autocheck {
                    self.run_check(false);
                }
                true
            }
            Err(e) => {
                self.err(format!("write failed: {}", e));
                false
            }
        }
    }

    fn set(&mut self, args: &str) {
        if args.is_empty() {
            let b = self.bb();
            let o = &self.opts;
            let s = format!(
                "{}number {}relativenumber ts={} sw={} {}expandtab {}autocheck {}autocomplete {}hlsearch {}list {}mouse ft={}",
                if o.number { "" } else { "no" },
                if o.relnum { "" } else { "no" },
                o.tabstop,
                b.indent_w,
                if b.expand_tab { "" } else { "no" },
                if o.autocheck { "" } else { "no" },
                if o.autocomplete { "" } else { "no" },
                if o.hlsearch { "" } else { "no" },
                if o.list { "" } else { "no" },
                if o.mouse { "" } else { "no" },
                b.lang.name
            );
            self.info(s);
            return;
        }
        for a in args.split_whitespace() {
            let (key, val) = match a.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (a, None),
            };
            let (key, on) = if let Some(k) = key.strip_suffix('!') {
                (k, None)
            } else if let Some(k) = key.strip_prefix("no") {
                (k, Some(false))
            } else {
                (key, Some(true))
            };
            let num = val.and_then(|v| v.parse::<usize>().ok());
            macro_rules! flag {
                ($f:expr) => {{
                    $f = match on {
                        Some(v) => v,
                        None => !$f,
                    }
                }};
            }
            match key {
                "nu" | "number" => flag!(self.opts.number),
                "rnu" | "relativenumber" => flag!(self.opts.relnum),
                "ac" | "autocheck" => flag!(self.opts.autocheck),
                "acp" | "autocomplete" => flag!(self.opts.autocomplete),
                "hls" | "hlsearch" => flag!(self.opts.hlsearch),
                "list" => flag!(self.opts.list),
                "mouse" => {
                    flag!(self.opts.mouse);
                    mtui::term::set_mouse(self.opts.mouse);
                }
                "et" | "expandtab" => flag!(self.b().expand_tab),
                "ts" | "tabstop" => {
                    if let Some(n) = num.filter(|&n| n > 0 && n <= 32) {
                        self.opts.tabstop = n;
                    }
                }
                "sw" | "shiftwidth" | "sts" | "softtabstop" => {
                    if let Some(n) = num.filter(|&n| n > 0 && n <= 32) {
                        self.b().indent_w = n;
                    }
                }
                "ft" | "filetype" | "syntax" => match val.and_then(crate::syntax::by_name) {
                    Some(l) => {
                        let b = self.b();
                        b.lang = l;
                        b.hl = Default::default();
                    }
                    None => self.err(format!("unknown filetype: {}", val.unwrap_or(""))),
                },
                _ => self.err(format!("unknown option: {}", key)),
            }
        }
    }

    fn substitute(&mut self, r1: usize, r2: usize, args: &str) {
        let (pat, rep, flags) = if args.is_empty() {
            (self.search_pat.clone(), self.regs.get(&'&').map(|r| r.text.clone()).unwrap_or_default(), String::new())
        } else {
            let d = args.chars().next().unwrap();
            let mut parts: Vec<String> = Vec::new();
            let mut cur = String::new();
            let mut it = args[d.len_utf8()..].chars();
            while let Some(c) = it.next() {
                if c == '\\' {
                    match it.next() {
                        Some(n) if n == d => cur.push(n),
                        Some(n) => {
                            cur.push('\\');
                            cur.push(n);
                        }
                        None => cur.push('\\'),
                    }
                } else if c == d && parts.len() < 2 {
                    parts.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
            parts.push(cur);
            let pat = parts.first().cloned().unwrap_or_default();
            let rep = parts.get(1).cloned().unwrap_or_default();
            let flags = parts.get(2).cloned().unwrap_or_default();
            (if pat.is_empty() { self.search_pat.clone() } else { pat }, rep, flags)
        };
        if pat.is_empty() {
            self.err("no previous pattern");
            return;
        }
        let global = flags.contains('g');
        let re = if flags.contains('I') {
            Regex::new(&pat, false)
        } else if flags.contains('i') {
            Regex::new(&pat.to_lowercase(), true)
        } else {
            Regex::new(&pat, true)
        };
        let re = match re {
            Ok(r) => r,
            Err(e) => return self.err(format!("bad pattern: {}", e)),
        };
        self.regs.insert('&', crate::editor::Reg { text: rep.clone(), line: false });
        let mut count = 0;
        let mut last_line = None;
        // bottom-up so inserted newlines don't shift unprocessed lines
        for ln in (r1..=r2.min(self.bb().lines.len() - 1)).rev() {
            let line = self.bb().lines[ln].clone();
            let mut out = String::new();
            let mut pos = 0;
            let mut changed = false;
            while pos <= line.len() {
                let Some(caps) = re.captures_at(&line, pos) else { break };
                let (a, z) = caps[0];
                out.push_str(&line[pos..a]);
                out.push_str(&re.expand(&line, &caps, &rep));
                count += 1;
                changed = true;
                if z == a {
                    match line[z..].chars().next() {
                        Some(c) => {
                            out.push(c);
                            pos = z + c.len_utf8();
                        }
                        None => {
                            pos = z + 1;
                            break;
                        }
                    }
                } else {
                    pos = z;
                }
                if !global {
                    break;
                }
            }
            if changed {
                if pos <= line.len() {
                    out.push_str(&line[pos..]);
                }
                let len = line.len();
                self.b().delete((ln, 0), (ln, len));
                self.b().insert((ln, 0), &out);
                last_line.get_or_insert(ln);
            }
        }
        self.search_pat = pat.clone();
        self.search = Some(re);
        if count == 0 {
            self.err(format!("pattern not found: {}", pat));
        } else {
            if let Some(l) = last_line {
                self.set_cursor((l, 0));
            }
            self.info(format!("{} substitution(s)", count));
        }
    }

    fn global(&mut self, (r1, r2): (usize, usize), args: &str, invert: bool) {
        let args = args.strip_prefix('!').unwrap_or(args);
        let Some(d) = args.chars().next() else { return };
        let body = &args[d.len_utf8()..];
        let (pat, cmd) = match body.find(d) {
            Some(i) => (&body[..i], &body[i + d.len_utf8()..]),
            None => (body, "p"),
        };
        let re = match Regex::new(pat, true) {
            Ok(r) => r,
            Err(e) => return self.err(format!("bad pattern: {}", e)),
        };
        let cmd = if cmd.trim().is_empty() || cmd.trim() == "p" { "p" } else { cmd.trim() };
        let hits: Vec<usize> = (r1..=r2).filter(|&l| re.find_at(&self.bb().lines[l], 0).is_some() != invert).collect();
        if cmd == "p" {
            self.msg_lines = hits.iter().map(|&l| format!("{:>5} {}", l + 1, self.bb().lines[l])).collect();
            return;
        }
        let n = hits.len();
        for &l in hits.iter().rev() {
            if l < self.bb().lines.len() {
                self.ex(&format!("{}{}", l + 1, cmd));
            }
        }
        self.info(format!("{} line(s) matched", n));
    }

    fn filter(&mut self, r1: usize, r2: usize, cmd: &str) {
        let input = self.bb().lines[r1..=r2].join("\n") + "\n";
        let out = run_shell(cmd, Some(&input));
        let out = out.strip_suffix('\n').unwrap_or(&out).to_string();
        let e = self.bb().line_len(r2);
        self.b().delete((r1, 0), (r2, e));
        self.b().insert((r1, 0), &out);
        self.set_cursor((r1, 0));
        self.info(format!("{} lines filtered", r2 - r1 + 1));
    }
}

fn expand_home(s: &str) -> PathBuf {
    if s == "~" {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default());
    }
    match s.strip_prefix("~/") {
        Some(r) => PathBuf::from(format!("{}/{}", std::env::var("HOME").unwrap_or_default(), r)),
        None => PathBuf::from(s),
    }
}

pub fn run_shell(cmd: &str, input: Option<&str>) -> String {
    let child = Command::new("sh").arg("-c").arg(format!("{} 2>&1", cmd)).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return e.to_string(),
    };
    if let (Some(inp), Some(mut si)) = (input, child.stdin.take()) {
        let data = inp.to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(data.as_bytes());
        });
    }
    match child.wait_with_output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => e.to_string(),
    }
}
