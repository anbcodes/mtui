// Markdown-aware reflow, for an editor's `gq` / `gw`. Only prose is wrapped:
// code (fenced or indented), front matter, tables, headings, rules, HTML,
// math blocks, link definitions and setext headings pass through untouched.
// Paragraphs keep their quote (`>`) and list-item prefixes, with hanging
// indents; inline code, links, images, autolinks and `$math$` are never
// split; hard line breaks are kept; and a wrapped line never starts with
// something that would turn it into a list, heading or quote.

use mtui::wrap::str_width;

/// Wrap lines `l1..=l2` of `lines` (the whole document, so that code fences
/// opened above the range are known) to `width` columns; returns the
/// replacement for just those lines.
pub fn format_range(lines: &[String], l1: usize, l2: usize, width: usize) -> Vec<String> {
    let l2 = l2.min(lines.len().saturating_sub(1));
    if lines.is_empty() || l1 > l2 {
        return Vec::new();
    }
    let verb = classify(lines);
    let mut out = Vec::new();
    let mut i = l1;
    while i <= l2 {
        let l = &lines[i];
        if verb[i] {
            out.push(if l.trim().is_empty() { String::new() } else { l.clone() });
            i += 1;
            continue;
        }
        let (first, cont, depth, body) = prefix(l);
        let mut segs: Vec<Vec<String>> = vec![tokens(body)];
        let mut hard: Vec<String> = vec![hard_break(l)];
        let mut j = i + 1;
        while j <= l2 && !verb[j] {
            let (d, at) = quotes(&lines[j]);
            let rest = &lines[j][at..];
            if d > depth || rest.trim().is_empty() || starts_block(rest) {
                break;
            }
            if !hard.last().unwrap().is_empty() {
                segs.push(Vec::new());
                hard.push(String::new());
            }
            *hard.last_mut().unwrap() = hard_break(&lines[j]);
            segs.last_mut().unwrap().extend(tokens(rest.trim()));
            j += 1;
        }
        for (k, seg) in segs.iter().enumerate() {
            let mut lines_out = wrap(seg, &first, &cont, width);
            if let Some(last) = lines_out.last_mut() {
                last.push_str(&hard[k]);
            }
            out.extend(lines_out);
        }
        i = j;
    }
    out
}

/// Per line: is it something to leave exactly as it is?
fn classify(lines: &[String]) -> Vec<bool> {
    let n = lines.len();
    let mut v = vec![false; n];
    let mut i = 0;
    if lines.first().is_some_and(|l| l.trim_end() == "---") {
        if let Some(j) = (1..n).find(|&j| matches!(lines[j].trim_end(), "---" | "...")) {
            v[..=j].iter_mut().for_each(|x| *x = true);
            i = j + 1;
        }
    }
    #[derive(PartialEq)]
    enum H {
        No,
        Block,
        Comment,
    }
    let (mut fence, mut html, mut math) = (None::<(char, usize)>, H::No, false);
    let (mut list_ctx, mut prev_blank) = (false, true);
    while i < n {
        let l = &lines[i];
        let t = l.trim();
        let (_, at) = quotes(l);
        let r = &l[at..];
        if let Some((c, len)) = fence {
            v[i] = true;
            let rt = r.trim();
            if rt.len() >= len && rt.chars().all(|x| x == c) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if math {
            v[i] = true;
            math = t != "$$";
            i += 1;
            continue;
        }
        if html != H::No {
            v[i] = true;
            if (html == H::Block && t.is_empty()) || (html == H::Comment && t.contains("-->")) {
                html = H::No;
            }
            i += 1;
            continue;
        }
        if r.trim().is_empty() {
            v[i] = true;
            prev_blank = true;
            i += 1;
            continue;
        }
        let rt = r.trim_start();
        if let Some(f) = fence_open(rt) {
            v[i] = true;
            fence = Some(f);
        } else if t == "$$" {
            v[i] = true;
            math = true;
        } else if rt.starts_with("<!--") {
            v[i] = true;
            if !rt.contains("-->") {
                html = H::Comment;
            }
        } else if html_line(rt) {
            v[i] = true;
            if !rt.ends_with('>') {
                html = H::Block;
            }
        } else if prev_blank && !list_ctx && l.starts_with("    ") {
            v[i] = true;
            prev_blank = true;
            i += 1;
            continue;
        } else if is_heading(rt) || is_rule(rt) || is_linkdef(rt) {
            v[i] = true;
        } else if l.contains('|') && lines.get(i + 1).is_some_and(|n| is_delim_row(n)) {
            let mut j = i;
            while j < n && lines[j].contains('|') && !lines[j].trim().is_empty() {
                v[j] = true;
                j += 1;
            }
            i = j;
            prev_blank = false;
            continue;
        } else if i + 1 < n && setext(&lines[i + 1]) && list_marker(rt).is_none() && !rt.starts_with('>') {
            v[i] = true;
            v[i + 1] = true;
            i += 2;
            prev_blank = false;
            continue;
        }
        if list_marker(rt).is_some() {
            list_ctx = true;
        } else if !l.starts_with(' ') && !l.starts_with('>') {
            list_ctx = false;
        }
        prev_blank = false;
        i += 1;
    }
    v
}

/// (depth, byte offset after the quote markers)
fn quotes(l: &str) -> (usize, usize) {
    let (mut d, mut at) = (0, 0);
    loop {
        let rest = &l[at..];
        let sp = rest.len() - rest.trim_start_matches(' ').len();
        if sp <= 3 && rest[sp..].starts_with('>') {
            at += sp + 1;
            if l[at..].starts_with(' ') {
                at += 1;
            }
            d += 1;
        } else {
            return (d, at);
        }
    }
}

fn fence_open(s: &str) -> Option<(char, usize)> {
    let c = s.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = s.chars().take_while(|&x| x == c).count();
    (n >= 3 && !(c == '`' && s[n..].contains('`'))).then_some((c, n))
}

fn html_line(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() > 2 && b[0] == b'<' && (b[1].is_ascii_alphabetic() || matches!(b[1], b'/' | b'!' | b'?')) && s.trim_end().ends_with('>')
}

fn is_heading(s: &str) -> bool {
    let n = s.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&n) && (s.len() == n || s[n..].starts_with([' ', '\t']))
}

fn is_rule(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && matches!(t.chars().next(), Some('-' | '*' | '_')) && t.chars().all(|c| Some(c) == t.chars().next())
}

fn is_linkdef(s: &str) -> bool {
    s.starts_with('[') && !s.starts_with("[^") && s.find("]:").is_some_and(|i| i > 1)
}

fn is_delim_row(l: &str) -> bool {
    let t = l.trim().trim_matches('|');
    !t.is_empty() && l.contains('-') && t.split('|').all(|c| {
        let c = c.trim().trim_start_matches(':').trim_end_matches(':');
        !c.is_empty() && c.chars().all(|x| x == '-')
    })
}

fn setext(l: &str) -> bool {
    let t = l.trim();
    !t.is_empty() && !l.starts_with("    ") && (t.chars().all(|c| c == '=') || (t.chars().all(|c| c == '-')))
}

/// Length of a list marker with its following spaces, and the marker alone.
fn list_marker(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let m = if matches!(b.first(), Some(b'-' | b'*' | b'+')) {
        1
    } else {
        let d = b.iter().take_while(|x| x.is_ascii_digit()).count();
        if (1..=9).contains(&d) && matches!(b.get(d), Some(b'.' | b')')) {
            d + 1
        } else {
            return None;
        }
    };
    if m < b.len() && b[m] != b' ' && b[m] != b'\t' {
        return None;
    }
    let sp = s[m..].len() - s[m..].trim_start_matches(' ').len();
    let sp = if sp == 0 || sp > 4 { 1 } else { sp };
    Some((m + sp, m))
}

fn starts_block(rest: &str) -> bool {
    let t = rest.trim_start();
    list_marker(t).is_some() || is_heading(t) || is_rule(t) || fence_open(t).is_some() || t.starts_with("[^") && t.contains("]:") || html_line(t) || t.starts_with("<!--") || t == "$$"
}

/// (first-line prefix, continuation prefix, quote depth, text after them)
fn prefix(l: &str) -> (String, String, usize, &str) {
    let (depth, at) = quotes(l);
    let rest = &l[at..];
    let ws = rest.len() - rest.trim_start().len();
    let (lead, body) = rest.split_at(ws);
    let quote = &l[..at];
    if let Some((full, m)) = list_marker(body) {
        let mut first = format!("{}{}{}{}", quote, lead, &body[..m], " ".repeat(full - m));
        let mut after = &body[body.len().min(m + (body[m..].len() - body[m..].trim_start_matches(' ').len()))..];
        if after.len() >= 4 && (after.starts_with("[ ] ") || after.starts_with("[x] ") || after.starts_with("[X] ")) {
            first.push_str(&after[..4]);
            after = &after[4..];
        }
        let cont = format!("{}{}{}", quote, lead, " ".repeat(full));
        return (first, cont, depth, after.trim());
    }
    if body.starts_with("[^") {
        if let Some(e) = body.find("]:") {
            let first = format!("{}{}{} ", quote, lead, &body[..e + 2]);
            return (first, format!("{}{}    ", quote, lead), depth, body[e + 2..].trim());
        }
    }
    let p = format!("{}{}", quote, lead);
    (p.clone(), p, depth, body.trim())
}

/// Trailing hard-break marker of a line ("  " or "\"), or empty.
fn hard_break(l: &str) -> String {
    let t = l.trim_end_matches(' ');
    let spaces = &l[t.len()..];
    if spaces.len() >= 2 && !t.is_empty() {
        return spaces.to_string();
    }
    let bs = t.chars().rev().take_while(|&c| c == '\\').count();
    if bs % 2 == 1 {
        return "\\".into();
    }
    String::new()
}

/// Split into words that must stay whole: code spans, links with their
/// destinations, autolinks / inline HTML and inline math.
fn tokens(s: &str) -> Vec<String> {
    let c: Vec<char> = s.trim_end_matches(|x| x == ' ').trim_end_matches('\\').chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            i += 1;
            continue;
        }
        let take = |end: usize, cur: &mut String, i: &mut usize| {
            cur.extend(&c[*i..end]);
            *i = end;
        };
        match ch {
            '\\' if i + 1 < c.len() => take(i + 2, &mut cur, &mut i),
            '`' => {
                let n = c[i..].iter().take_while(|&&x| x == '`').count();
                let mut j = i + n;
                let mut found = None;
                while j < c.len() {
                    if c[j] == '`' {
                        let r = c[j..].iter().take_while(|&&x| x == '`').count();
                        if r == n {
                            found = Some(j + n);
                            break;
                        }
                        j += r;
                    } else {
                        j += 1;
                    }
                }
                take(found.unwrap_or(i + n), &mut cur, &mut i);
            }
            ']' if c.get(i + 1) == Some(&'(') => {
                let mut depth = 0;
                let mut j = i + 1;
                while j < c.len() {
                    match c[j] {
                        '\\' => j += 1,
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                take((j + 1).min(c.len()), &mut cur, &mut i);
            }
            '<' if c.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || matches!(x, '/' | '!')) => match c[i..].iter().position(|&x| x == '>') {
                Some(e) => take(i + e + 1, &mut cur, &mut i),
                None => take(i + 1, &mut cur, &mut i),
            },
            '$' if c.get(i + 1).is_some_and(|x| !x.is_whitespace() && *x != '$') => match (i + 1..c.len()).find(|&j| c[j] == '$' && !c[j - 1].is_whitespace() && c[j - 1] != '\\') {
                Some(e) => take(e + 1, &mut cur, &mut i),
                None => take(i + 1, &mut cur, &mut i),
            },
            _ => take(i + 1, &mut cur, &mut i),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Words that would start some other block if they began a line.
fn risky(w: &str) -> bool {
    let digits = w.trim_end_matches(['.', ')']);
    matches!(w, "-" | "+" | "*")
        || w.starts_with('>')
        || (w.chars().all(|c| c == '#') && w.len() <= 6)
        || (digits.len() < w.len() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        || (w.len() >= 3 && (w.chars().all(|c| c == '-') || w.chars().all(|c| c == '=')))
        || w.starts_with("```")
        || w.starts_with("~~~")
        || w.starts_with('|')
        || w == "$$"
}

fn wrap(words: &[String], first: &str, cont: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = first.to_string();
    let mut has = false;
    for w in words {
        let ww = str_width(w);
        if has && str_width(&cur) + 1 + ww > width && !risky(w) {
            out.push(std::mem::replace(&mut cur, cont.to_string()));
            has = false;
        }
        if has {
            cur.push(' ');
        }
        cur.push_str(w);
        has = true;
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(s: &str, w: usize) -> String {
        let lines: Vec<String> = s.split('\n').map(String::from).collect();
        let out = format_range(&lines, 0, lines.len() - 1, w).join("\n");
        // formatting is idempotent
        let again: Vec<String> = out.split('\n').map(String::from).collect();
        assert_eq!(format_range(&again, 0, again.len() - 1, w).join("\n"), out, "not idempotent");
        out
    }

    #[test]
    fn paragraphs_wrap_and_join() {
        assert_eq!(fmt("aaa bbb ccc\nddd eee\n\nnext para here", 10), "aaa bbb\nccc ddd\neee\n\nnext para\nhere");
    }

    #[test]
    fn lists_get_hanging_indents() {
        assert_eq!(fmt("- one two three four\n  - nested item with words\n1. numbered item that is long", 14), "- one two\n  three four\n  - nested\n    item with\n    words\n1. numbered\n   item that\n   is long");
    }

    #[test]
    fn quotes_keep_their_marker() {
        assert_eq!(fmt("> quoted text that goes on\n> and on\n> > nested quote text here", 14), "> quoted text\n> that goes on\n> and on\n> > nested\n> > quote text\n> > here");
    }

    #[test]
    fn code_tables_headings_are_untouched() {
        let s = "# A very long heading that stays on one line\n\n```\ncode   with   spacing and a very long line that stays\n```\n\n| a | b |\n|---|---|\n| long cell text here | x |\n\n    indented code line stays as is\n\n---\ntitle\n=====";
        assert_eq!(fmt(s, 12).split("\n\n").next().unwrap(), "# A very long heading that stays on one line");
        let out = fmt(s, 12);
        assert!(out.contains("code   with   spacing and a very long line that stays"));
        assert!(out.contains("| long cell text here | x |"));
        assert!(out.contains("    indented code line stays as is"));
        assert!(out.ends_with("title\n====="));
    }

    #[test]
    fn inline_constructs_stay_whole() {
        assert_eq!(fmt("see `a code span` and [a link](http://x.y/z w \"t\") ok", 12), "see\n`a code span`\nand [a\nlink](http://x.y/z w \"t\")\nok");
    }

    #[test]
    fn never_starts_a_line_with_a_marker() {
        assert_eq!(fmt("aaa bbb - ccc 2. ddd # eee", 7), "aaa bbb -\nccc 2.\nddd #\neee");
    }

    #[test]
    fn hard_breaks_and_tasks() {
        assert_eq!(fmt("one two  \nthree four\\\nfive\n\n- [ ] task with a long description", 12), "one two  \nthree four\\\nfive\n\n- [ ] task\n  with a\n  long\n  description");
    }

    #[test]
    fn front_matter_and_fences_above_the_range_are_respected() {
        let lines: Vec<String> = "---\ntitle: x\n---\n\n```\nlong code line that must stay put even when selected\n```".split('\n').map(String::from).collect();
        let out = format_range(&lines, 5, 5, 10);
        assert_eq!(out, vec![lines[5].clone()]);
        assert_eq!(format_range(&lines, 1, 1, 3), vec!["title: x".to_string()]);
    }
}
