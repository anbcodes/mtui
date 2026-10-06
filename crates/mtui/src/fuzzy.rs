// Fuzzy subsequence matching, shared by pickers and completion.

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Fuzzy subsequence score; higher is better. None if no match.
pub fn fuzzy(pat: &str, s: &str) -> Option<i32> {
    if pat.is_empty() {
        return Some(0);
    }
    let mut score = 0i32;
    let mut pi = pat.chars().peekable();
    let mut prev_match = false;
    let mut prev_c = ' ';
    let mut first: Option<usize> = None;
    for (i, c) in s.chars().enumerate() {
        let Some(&p) = pi.peek() else { break };
        if c.eq_ignore_ascii_case(&p) {
            first.get_or_insert(i);
            score += 10;
            if prev_match {
                score += 15;
            }
            if i == 0 || !is_word_char(prev_c) || (c.is_uppercase() && prev_c.is_lowercase()) || prev_c == '/' {
                score += 20;
            }
            if c == p {
                score += 2;
            }
            prev_match = true;
            pi.next();
        } else {
            prev_match = false;
            score -= 1;
        }
        prev_c = c;
    }
    if pi.peek().is_some() {
        return None;
    }
    Some(score - first.unwrap_or(0) as i32 * 2 - (s.len() as i32 / 8))
}

/// Indices of `labels` that match `query` (spaces ignored), best first.
/// An empty query keeps the original order.
pub fn filter<'a>(query: &str, labels: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let q = query.replace(' ', "");
    if q.is_empty() {
        return labels.enumerate().map(|(i, _)| i).collect();
    }
    let mut v: Vec<(i32, usize)> = labels.enumerate().filter_map(|(i, l)| fuzzy(&q, l).map(|s| (s, i))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|x| x.1).collect()
}
