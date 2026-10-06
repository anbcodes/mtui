// Raw terminal handling and key decoding. Uses only libc.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ORIG: Mutex<Option<libc::termios>> = Mutex::new(None);
pub static RESIZED: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: libc::c_int) {
    RESIZED.store(true, Ordering::Relaxed);
}

pub const ENTER_SEQ: &str = "\x1b[?1049h\x1b[?7l\x1b[?2004h\x1b[H\x1b[2J";
pub const LEAVE_SEQ: &str = "\x1b[0m\x1b[2 q\x1b[?25h\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?7h\x1b[?1049l";
/// Press/release, drag and wheel reporting, SGR-encoded (no 223-column limit).
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// Turn mouse events (`Key::Mouse`) on or off; takes effect immediately in
/// raw mode, otherwise at the next `enable_raw`. While enabled, most
/// terminals still select text with Shift+drag.
pub fn set_mouse(on: bool) {
    MOUSE.store(on, Ordering::Relaxed);
    if ACTIVE.load(Ordering::Relaxed) {
        let mut o = io::stdout();
        let _ = o.write_all(if on { MOUSE_ON } else { MOUSE_OFF }.as_bytes());
        let _ = o.flush();
    }
}

pub fn enable_raw() -> io::Result<()> {
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return Err(io::Error::last_os_error());
        }
        *ORIG.lock().unwrap() = Some(t);
        libc::cfmakeraw(&mut t);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(0, libc::TCSAFLUSH, &t);
        libc::signal(libc::SIGWINCH, on_winch as *const () as libc::sighandler_t);
    }
    let mut o = io::stdout();
    o.write_all(ENTER_SEQ.as_bytes())?;
    if MOUSE.load(Ordering::Relaxed) {
        o.write_all(MOUSE_ON.as_bytes())?;
    }
    ACTIVE.store(true, Ordering::Relaxed);
    o.flush()
}

pub fn disable_raw() {
    ACTIVE.store(false, Ordering::Relaxed);
    let mut o = io::stdout();
    if crate::kitty::USED.load(Ordering::Relaxed) {
        let _ = o.write_all(crate::kitty::FREE_ALL);
    }
    let _ = o.write_all(LEAVE_SEQ.as_bytes());
    let _ = o.flush();
    if let Some(t) = *ORIG.lock().unwrap() {
        unsafe {
            libc::tcsetattr(0, libc::TCSAFLUSH, &t);
        }
    }
}

/// Restore the terminal before printing a panic message.
pub fn install_panic_hook(name: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        disable_raw();
        eprintln!("{} crashed: {}", name, info);
    }));
}

/// Ctrl-Z: give the terminal back, stop, and re-enter raw mode on resume.
pub fn suspend() {
    disable_raw();
    unsafe {
        libc::kill(0, libc::SIGTSTP);
    }
    let _ = enable_raw();
}

pub fn is_tty() -> bool {
    unsafe { libc::isatty(0) != 0 && libc::isatty(1) != 0 }
}

/// A self-pipe that lets background threads interrupt `Input::next_key`.
#[derive(Clone, Copy)]
pub struct Waker {
    rd: i32,
    wr: i32,
}

impl Waker {
    pub fn new() -> Waker {
        let mut fds = [0i32; 2];
        unsafe {
            libc::pipe(fds.as_mut_ptr());
            for fd in fds {
                libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        Waker { rd: fds[0], wr: fds[1] }
    }

    pub fn wake(&self) {
        unsafe {
            libc::write(self.wr, b"x".as_ptr() as *const libc::c_void, 1);
        }
    }

    fn drain(&self) {
        let mut b = [0u8; 64];
        while unsafe { libc::read(self.rd, b.as_mut_ptr() as *mut libc::c_void, b.len()) } > 0 {}
    }
}

/// Size of one cell in pixels, if the terminal reports it.
pub fn cell_px() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
            (ws.ws_xpixel as usize / ws.ws_col as usize, ws.ws_ypixel as usize / ws.ws_row as usize)
        } else {
            (8, 16)
        }
    }
}

pub fn size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            (ws.ws_col as usize, ws.ws_row as usize)
        } else {
            (80, 24)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    Esc,
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Paste(String),
    Mouse(Mouse),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    /// 0 = left, 1 = middle, 2 = right
    Press(u8),
    /// Motion with a button held.
    Drag(u8),
    Release,
    WheelUp,
    WheelDown,
}

/// A mouse event at a 0-based screen cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub x: usize,
    pub y: usize,
}

pub struct Input {
    buf: Vec<u8>,
    pos: usize,
    waker: Option<Waker>,
}

fn poll_fd(timeout_ms: i32) -> bool {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut p, 1, timeout_ms) > 0 }
}

/// Wait for stdin or the waker. True only if stdin is readable.
fn poll_stdin_or(w: &Waker, timeout_ms: i32) -> bool {
    let mut p = [libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: w.rd, events: libc::POLLIN, revents: 0 }];
    if unsafe { libc::poll(p.as_mut_ptr(), 2, timeout_ms) } <= 0 {
        return false;
    }
    if p[1].revents != 0 {
        w.drain();
    }
    p[0].revents != 0
}

impl Input {
    pub fn new() -> Self {
        Input { buf: Vec::with_capacity(256), pos: 0, waker: None }
    }

    /// Let `next_key` return None early when `w.wake()` is called.
    pub fn with_waker(mut self, w: Waker) -> Self {
        self.waker = Some(w);
        self
    }

    fn avail(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Read more bytes from stdin, waiting up to `timeout_ms` (-1 = forever).
    fn fill(&mut self, timeout_ms: i32) -> bool {
        if !poll_fd(timeout_ms) {
            return false;
        }
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let mut tmp = [0u8; 4096];
        let n = unsafe { libc::read(0, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
        if n <= 0 {
            return false;
        }
        self.buf.extend_from_slice(&tmp[..n as usize]);
        true
    }

    /// True if a key is ready without blocking.
    pub fn ready(&mut self) -> bool {
        self.avail() > 0 || poll_fd(0)
    }

    fn need(&mut self, n: usize) -> bool {
        while self.avail() < n {
            if !self.fill(50) {
                return false;
            }
        }
        true
    }

    fn peek(&self, i: usize) -> u8 {
        self.buf[self.pos + i]
    }

    pub fn next_key(&mut self, timeout_ms: i32) -> Option<Key> {
        if self.avail() == 0 {
            if let Some(w) = self.waker {
                if !poll_stdin_or(&w, timeout_ms) {
                    return None;
                }
            }
            if !self.fill(timeout_ms) {
                return None;
            }
        }
        let b = self.peek(0);
        if b == 0x1b {
            self.need(2);
            if self.avail() < 2 {
                self.pos += 1;
                return Some(Key::Esc);
            }
            let b1 = self.peek(1);
            if b1 == b'[' || b1 == b'O' {
                return Some(self.parse_csi(b1 == b'O'));
            }
            if b1 == 0x1b {
                self.pos += 1;
                return Some(Key::Esc);
            }
            self.pos += 1;
            return match self.decode() {
                Key::Char(c) => Some(Key::Alt(c)),
                Key::Enter => Some(Key::Alt('\n')),
                k => Some(k),
            };
        }
        Some(self.decode())
    }

    fn decode(&mut self) -> Key {
        let b = self.peek(0);
        let k = match b {
            b'\r' | b'\n' => Key::Enter,
            b'\t' => Key::Tab,
            0x7f | 0x08 => Key::Backspace,
            0 => Key::Ctrl(' '),
            1..=26 => Key::Ctrl((b'a' + b - 1) as char),
            0x1b => Key::Esc,
            28..=31 => Key::Ctrl((b'4' + b - 28) as char),
            _ if b < 0x80 => Key::Char(b as char),
            _ => {
                let len = if b >= 0xf0 { 4 } else if b >= 0xe0 { 3 } else { 2 };
                if !self.need(len) {
                    self.pos = self.buf.len();
                    return Key::Char('\u{fffd}');
                }
                let s = &self.buf[self.pos..self.pos + len];
                let c = std::str::from_utf8(s).ok().and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
                self.pos += len;
                return Key::Char(c);
            }
        };
        self.pos += 1;
        k
    }

    fn parse_csi(&mut self, ss3: bool) -> Key {
        // buf[pos] = ESC, buf[pos+1] = '[' or 'O'
        let mut i = 2;
        loop {
            if !self.need(i + 1) {
                self.pos = self.buf.len();
                return Key::Esc;
            }
            let c = self.peek(i);
            if (0x40..=0x7e).contains(&c) && !(ss3 == false && i == 2 && c == b'[') {
                break;
            }
            i += 1;
            if i > 32 {
                self.pos += i;
                return Key::Esc;
            }
        }
        let fin = self.peek(i);
        let params: String = String::from_utf8_lossy(&self.buf[self.pos + 2..self.pos + i]).into();
        self.pos += i + 1;
        if let Some(p) = params.strip_prefix('<') {
            return parse_sgr_mouse(p, fin == b'm');
        }
        let first: u32 = params.split(';').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        match fin {
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => Key::Right,
            b'D' => Key::Left,
            b'H' => Key::Home,
            b'F' => Key::End,
            b'Z' => Key::BackTab,
            b'~' => match first {
                1 | 7 => Key::Home,
                4 | 8 => Key::End,
                3 => Key::Delete,
                5 => Key::PageUp,
                6 => Key::PageDown,
                200 => self.read_paste(),
                _ => Key::Esc,
            },
            _ => Key::Esc,
        }
    }

    fn read_paste(&mut self) -> Key {
        let end = b"\x1b[201~";
        let mut out = Vec::new();
        loop {
            if self.avail() == 0 && !self.fill(2000) {
                break;
            }
            let s = &self.buf[self.pos..];
            if let Some(i) = s.windows(end.len()).position(|w| w == end) {
                out.extend_from_slice(&s[..i]);
                self.pos += i + end.len();
                break;
            }
            // keep a tail in case the terminator is split across reads
            let keep = (end.len() - 1).min(s.len());
            let take = s.len() - keep;
            out.extend_from_slice(&s[..take]);
            self.pos += take;
            if !self.fill(2000) {
                out.extend_from_slice(&self.buf[self.pos..]);
                self.pos = self.buf.len();
                break;
            }
        }
        let s = String::from_utf8_lossy(&out).replace("\r\n", "\n").replace('\r', "\n");
        Key::Paste(s)
    }
}

/// `ESC [ < b ; x ; y M` (press) or `m` (release), with 1-based x/y.
fn parse_sgr_mouse(p: &str, release: bool) -> Key {
    let v: Vec<usize> = p.split(';').map(|s| s.parse().unwrap_or(0)).collect();
    let [b, x, y] = v[..] else { return Key::Esc };
    let kind = match (b & !(4 | 8 | 16), release) {
        (64, _) => MouseKind::WheelUp,
        (65, _) => MouseKind::WheelDown,
        (_, true) => MouseKind::Release,
        (b, false) if b < 3 => MouseKind::Press(b as u8),
        (b, false) if b & 32 != 0 && b & 3 < 3 => MouseKind::Drag((b & 3) as u8),
        _ => return Key::Esc, // motion, horizontal wheel, extra buttons
    };
    Key::Mouse(Mouse { kind, x: x.saturating_sub(1), y: y.saturating_sub(1) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_mouse() {
        assert_eq!(parse_sgr_mouse("0;10;5", false), Key::Mouse(Mouse { kind: MouseKind::Press(0), x: 9, y: 4 }));
        assert_eq!(parse_sgr_mouse("0;10;5", true), Key::Mouse(Mouse { kind: MouseKind::Release, x: 9, y: 4 }));
        assert_eq!(parse_sgr_mouse("65;1;1", false), Key::Mouse(Mouse { kind: MouseKind::WheelDown, x: 0, y: 0 }));
        assert_eq!(parse_sgr_mouse("16;3;3", false), Key::Mouse(Mouse { kind: MouseKind::Press(0), x: 2, y: 2 }));
        assert_eq!(parse_sgr_mouse("32;3;3", false), Key::Mouse(Mouse { kind: MouseKind::Drag(0), x: 2, y: 2 }));
        assert_eq!(parse_sgr_mouse("35;3;3", false), Key::Esc);
    }
}
