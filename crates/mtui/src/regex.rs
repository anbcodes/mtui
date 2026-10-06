// A tiny backtracking regex engine (PCRE-ish subset) used for search and :s.
// Supports: literals . [] [^] \w \d \s \W \D \S \b \B ^ $ * + ? {n,m} lazy ?
// groups ( ) (?: ) alternation | and captures \1..\9 in replacements.

use std::cell::Cell;

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    WordB(bool),
    Group(Vec<Vec<Node>>, Option<usize>),
    Repeat(Box<Node>, usize, usize, bool),
}

#[derive(Debug, Clone)]
enum ClassItem {
    Range(char, char),
    Word(bool),
    Digit(bool),
    Space(bool),
}

pub struct Regex {
    alts: Vec<Vec<Node>>,
    ncaps: usize,
    icase: bool,
    lit_prefix: Option<char>,
}

struct Parser<'a> {
    s: Vec<char>,
    i: usize,
    ncaps: usize,
    _p: std::marker::PhantomData<&'a ()>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn alts(&mut self) -> Result<Vec<Vec<Node>>, String> {
        let mut alts = vec![self.seq()?];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.seq()?);
        }
        Ok(alts)
    }
    fn seq(&mut self) -> Result<Vec<Node>, String> {
        let mut v = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            let atom = self.quant(atom)?;
            v.push(atom);
        }
        Ok(v)
    }
    fn quant(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let save = self.i;
                self.i += 1;
                let mut a = String::new();
                while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                    a.push(c);
                    self.i += 1;
                }
                let mut b = a.clone();
                if self.peek() == Some(',') {
                    self.i += 1;
                    b.clear();
                    while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                        b.push(c);
                        self.i += 1;
                    }
                }
                if self.peek() != Some('}') || a.is_empty() {
                    self.i = save;
                    return Ok(atom);
                }
                let min = a.parse().unwrap_or(0);
                let max = if b.is_empty() { usize::MAX } else { b.parse().unwrap_or(usize::MAX) };
                (min, max)
            }
            _ => return Ok(atom),
        };
        self.i += 1;
        let greedy = if self.peek() == Some('?') {
            self.i += 1;
            false
        } else {
            true
        };
        if matches!(atom, Node::Start | Node::End | Node::WordB(_)) {
            return Err("nothing to repeat".into());
        }
        Ok(Node::Repeat(Box::new(atom), min, max, greedy))
    }
    fn escape_class(c: char) -> Option<ClassItem> {
        Some(match c {
            'w' => ClassItem::Word(false),
            'W' => ClassItem::Word(true),
            'd' => ClassItem::Digit(false),
            'D' => ClassItem::Digit(true),
            's' => ClassItem::Space(false),
            'S' => ClassItem::Space(true),
            _ => return None,
        })
    }
    fn escape_char(c: char) -> char {
        match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            c => c,
        }
    }
    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().unwrap();
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let cap = if self.s.get(self.i) == Some(&'?') && self.s.get(self.i + 1) == Some(&':') {
                    self.i += 2;
                    None
                } else {
                    self.ncaps += 1;
                    Some(self.ncaps)
                };
                let alts = self.alts()?;
                if self.peek() != Some(')') {
                    return Err("missing )".into());
                }
                self.i += 1;
                Node::Group(alts, cap)
            }
            '[' => {
                let neg = self.peek() == Some('^');
                if neg {
                    self.i += 1;
                }
                let mut items = Vec::new();
                let mut first = true;
                loop {
                    let c = self.peek().ok_or("missing ]")?;
                    self.i += 1;
                    if c == ']' && !first {
                        break;
                    }
                    first = false;
                    let lo = if c == '\\' {
                        let e = self.peek().ok_or("bad escape")?;
                        self.i += 1;
                        if let Some(ci) = Self::escape_class(e) {
                            items.push(ci);
                            continue;
                        }
                        Self::escape_char(e)
                    } else {
                        c
                    };
                    if self.peek() == Some('-') && self.s.get(self.i + 1).map_or(false, |&c| c != ']') {
                        self.i += 1;
                        let mut hi = self.peek().unwrap();
                        self.i += 1;
                        if hi == '\\' {
                            hi = Self::escape_char(self.peek().ok_or("bad escape")?);
                            self.i += 1;
                        }
                        items.push(ClassItem::Range(lo, hi));
                    } else {
                        items.push(ClassItem::Range(lo, lo));
                    }
                }
                Node::Class(items, neg)
            }
            '\\' => {
                let e = self.peek().ok_or("trailing \\")?;
                self.i += 1;
                if let Some(ci) = Self::escape_class(e) {
                    Node::Class(vec![ci], false)
                } else if e == 'b' {
                    Node::WordB(true)
                } else if e == 'B' {
                    Node::WordB(false)
                } else if e == '<' || e == '>' {
                    Node::WordB(true)
                } else {
                    Node::Char(Self::escape_char(e))
                }
            }
            c => Node::Char(c),
        })
    }
}

pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

fn char_at(s: &str, i: usize) -> Option<char> {
    s.get(i..).and_then(|t| t.chars().next())
}

fn char_before(s: &str, i: usize) -> Option<char> {
    s.get(..i).and_then(|t| t.chars().next_back())
}

type Caps = [Cell<(usize, usize)>];

impl Regex {
    /// Compile. `smartcase`: case-insensitive unless the pattern has uppercase.
    pub fn new(pat: &str, smartcase: bool) -> Result<Regex, String> {
        let mut p = Parser { s: pat.chars().collect(), i: 0, ncaps: 0, _p: Default::default() };
        let alts = p.alts()?;
        if p.i < p.s.len() {
            return Err("unmatched )".into());
        }
        let icase = smartcase && !pat.chars().any(|c| c.is_uppercase());
        // first char every match must start with (ASCII only when case-insensitive)
        let lit_prefix = if alts.len() == 1 {
            match alts[0].first() {
                Some(Node::Char(c)) if !icase || c.is_ascii() => Some(if icase { c.to_ascii_lowercase() } else { *c }),
                _ => None,
            }
        } else {
            None
        };
        Ok(Regex { alts, ncaps: p.ncaps, icase, lit_prefix })
    }

    fn class_match(&self, items: &[ClassItem], neg: bool, c: char) -> bool {
        let lc = lower(c);
        let m = items.iter().any(|it| match *it {
            ClassItem::Range(a, b) => {
                (a <= c && c <= b) || (self.icase && ((lower(a) <= lc && lc <= lower(b)) || (a <= c.to_ascii_uppercase() && c.to_ascii_uppercase() <= b)))
            }
            ClassItem::Word(n) => is_word(c) != n,
            ClassItem::Digit(n) => c.is_ascii_digit() != n,
            ClassItem::Space(n) => c.is_whitespace() != n,
        });
        m != neg
    }

    /// Match a single-width node at i; returns the next index.
    fn single(&self, n: &Node, s: &str, i: usize) -> Option<usize> {
        match n {
            Node::Char(c) => {
                let d = char_at(s, i)?;
                if d == *c || (self.icase && lower(d) == lower(*c)) {
                    Some(i + d.len_utf8())
                } else {
                    None
                }
            }
            Node::Any => char_at(s, i).map(|d| i + d.len_utf8()),
            Node::Class(items, neg) => {
                let d = char_at(s, i)?;
                if self.class_match(items, *neg, d) {
                    Some(i + d.len_utf8())
                } else {
                    None
                }
            }
            Node::Start => (i == 0).then_some(i),
            Node::End => (i == s.len()).then_some(i),
            Node::WordB(want) => {
                let a = char_before(s, i).map_or(false, is_word);
                let b = char_at(s, i).map_or(false, is_word);
                ((a != b) == *want).then_some(i)
            }
            _ => None,
        }
    }

    fn m_seq(&self, seq: &[Node], s: &str, i: usize, caps: &Caps, k: &mut dyn FnMut(usize) -> bool) -> bool {
        let Some(n) = seq.first() else { return k(i) };
        let rest = &seq[1..];
        match n {
            Node::Group(alts, cap) => {
                for alt in alts {
                    let ok = self.m_seq(alt, s, i, caps, &mut |j| {
                        if let Some(ci) = cap {
                            let old = caps[*ci].get();
                            caps[*ci].set((i, j));
                            if self.m_seq(rest, s, j, caps, k) {
                                return true;
                            }
                            caps[*ci].set(old);
                            false
                        } else {
                            self.m_seq(rest, s, j, caps, k)
                        }
                    });
                    if ok {
                        return true;
                    }
                }
                false
            }
            Node::Repeat(node, min, max, greedy) => {
                // Fast path for simple atoms: collect positions iteratively.
                if !matches!(**node, Node::Group(..) | Node::Repeat(..)) {
                    let mut pos = vec![i];
                    let mut j = i;
                    while pos.len() - 1 < *max {
                        match self.single(node, s, j) {
                            Some(nj) if nj > j => {
                                j = nj;
                                pos.push(j);
                            }
                            _ => break,
                        }
                    }
                    if pos.len() - 1 < *min {
                        return false;
                    }
                    if *greedy {
                        for &p in pos[*min..].iter().rev() {
                            if self.m_seq(rest, s, p, caps, k) {
                                return true;
                            }
                        }
                    } else {
                        for &p in &pos[*min..] {
                            if self.m_seq(rest, s, p, caps, k) {
                                return true;
                            }
                        }
                    }
                    return false;
                }
                self.m_rep(node, *min, *max, *greedy, 0, rest, s, i, caps, k)
            }
            _ => match self.single(n, s, i) {
                Some(j) => self.m_seq(rest, s, j, caps, k),
                None => false,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn m_rep(&self, node: &Node, min: usize, max: usize, greedy: bool, count: usize, rest: &[Node], s: &str, i: usize, caps: &Caps, k: &mut dyn FnMut(usize) -> bool) -> bool {
        let one = std::slice::from_ref(node);
        let try_more = |k: &mut dyn FnMut(usize) -> bool| -> bool {
            count < max
                && self.m_seq(one, s, i, caps, &mut |j| j > i && self.m_rep(node, min, max, greedy, count + 1, rest, s, j, caps, k))
        };
        if count < min {
            return try_more(k);
        }
        if greedy {
            try_more(k) || self.m_seq(rest, s, i, caps, k)
        } else {
            self.m_seq(rest, s, i, caps, k) || try_more(k)
        }
    }

    /// Find a match starting at or after byte `start`. Returns capture spans (0 = whole match).
    pub fn captures_at(&self, s: &str, start: usize) -> Option<Vec<(usize, usize)>> {
        let caps: Vec<Cell<(usize, usize)>> = (0..=self.ncaps).map(|_| Cell::new((usize::MAX, usize::MAX))).collect();
        let mut i = start;
        while i <= s.len() {
            if let Some(c) = self.lit_prefix {
                let found = if self.icase {
                    let (lo, up) = (c as u8, c.to_ascii_uppercase() as u8);
                    s.as_bytes()[i..].iter().position(|&b| b == lo || b == up)
                } else {
                    s[i..].find(c)
                };
                match found {
                    Some(off) => i += off,
                    None => return None,
                }
            }
            let mut end = None;
            for alt in &self.alts {
                if self.m_seq(alt, s, i, &caps, &mut |j| {
                    end = Some(j);
                    true
                }) {
                    break;
                }
            }
            if let Some(e) = end {
                caps[0].set((i, e));
                return Some(caps.iter().map(|c| c.get()).collect());
            }
            match char_at(s, i) {
                Some(c) => i += c.len_utf8(),
                None => break,
            }
        }
        None
    }

    pub fn find_at(&self, s: &str, start: usize) -> Option<(usize, usize)> {
        self.captures_at(s, start).map(|c| c[0])
    }

    /// Last match starting strictly before `before`.
    pub fn rfind_before(&self, s: &str, before: usize) -> Option<(usize, usize)> {
        let mut last = None;
        let mut i = 0;
        while let Some((a, b)) = self.find_at(s, i) {
            if a >= before {
                break;
            }
            last = Some((a, b));
            i = if b > a { b } else { a + char_at(s, a).map_or(1, |c| c.len_utf8()) };
            if i > s.len() {
                break;
            }
        }
        last
    }

    /// Expand a replacement string: & or \0 = whole, \1..\9 = groups, \n newline.
    pub fn expand(&self, s: &str, caps: &[(usize, usize)], rep: &str) -> String {
        let mut out = String::new();
        let mut it = rep.chars().peekable();
        let get = |n: usize| -> &str {
            match caps.get(n) {
                Some(&(a, b)) if a != usize::MAX => &s[a..b],
                _ => "",
            }
        };
        while let Some(c) = it.next() {
            match c {
                '&' => out.push_str(get(0)),
                '\\' => match it.next() {
                    Some(d @ '0'..='9') => out.push_str(get(d as usize - '0' as usize)),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(d) => out.push(d),
                    None => out.push('\\'),
                },
                c => out.push(c),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(p: &str, s: &str) -> Option<(usize, usize)> {
        Regex::new(p, false).unwrap().find_at(s, 0)
    }
    #[test]
    fn basics() {
        assert_eq!(f("abc", "xxabcx"), Some((2, 5)));
        assert_eq!(f("a.c", "abc"), Some((0, 3)));
        assert_eq!(f("a*", "aaab"), Some((0, 3)));
        assert_eq!(f("^b", "ab"), None);
        assert_eq!(f("b$", "ab"), Some((1, 2)));
        assert_eq!(f("[0-9]+", "ab123c"), Some((2, 5)));
        assert_eq!(f("\\bfoo\\b", "afoo foo"), Some((5, 8)));
        assert_eq!(f("(ab)+c", "xababc"), Some((1, 6)));
        assert_eq!(f("cat|dog", "hotdog"), Some((3, 6)));
        assert_eq!(f("a{2,3}", "caaaa"), Some((1, 4)));
        assert_eq!(f("\".*?\"", "x \"a\" \"b\""), Some((2, 5)));
        assert_eq!(f("(a|ab)(c|bcd)(d*)", "abcd"), Some((0, 4)));
        let r = Regex::new("(\\w+)=(\\w+)", false).unwrap();
        let s = "key=val";
        let c = r.captures_at(s, 0).unwrap();
        assert_eq!(r.expand(s, &c, "\\2=\\1"), "val=key");
        assert_eq!(Regex::new("hello", true).unwrap().find_at("HeLLo", 0), Some((0, 5)));
    }
}
