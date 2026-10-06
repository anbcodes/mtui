// Word wrapping by display width.

use crate::screen::char_width;

pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Wrap `s` to `width` columns. Hard newlines are kept; words longer than a
/// line are split. Returns byte ranges into `s`, one per visual line.
pub fn wrap(s: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut base = 0;
    for para in s.split('\n') {
        let (mut start, mut w) = (0, 0);
        let mut brk: Option<usize> = None; // byte index just after the last space
        for (i, c) in para.char_indices() {
            if c == ' ' {
                if w + 1 > width {
                    out.push((base + start, base + i));
                    (start, w, brk) = (i + 1, 0, None);
                } else {
                    w += 1;
                    brk = Some(i + 1);
                }
                continue;
            }
            let cw = char_width(c);
            if w + cw > width && i > start {
                let end = match brk {
                    Some(b) if b > start => b,
                    _ => i,
                };
                out.push((base + start, base + start + para[start..end].trim_end_matches(' ').len()));
                start = end;
                w = str_width(&para[start..i]);
                brk = None;
            }
            w += cw;
        }
        out.push((base + start, base + para.len()));
        base += para.len() + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str, w: usize) -> Vec<&str> {
        wrap(s, w).into_iter().map(|(a, b)| &s[a..b]).collect()
    }

    #[test]
    fn basic() {
        assert_eq!(lines("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(lines("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(lines("a\n\nb", 5), vec!["a", "", "b"]);
        assert_eq!(lines("", 5), vec![""]);
    }
}
