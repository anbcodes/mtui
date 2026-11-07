//! Decorations a host program puts on a buffer without changing its text:
//! rows drawn between lines (a diff's deleted lines), a background and a
//! gutter mark per line (added lines), and folds that put one summary row in
//! place of a range of lines. mdiff uses them to show a diff inside the
//! editor while the text stays editable.

use crate::buffer::Buffer;

/// A row drawn above a line; not part of the text.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Virt {
    pub text: String,
    /// 0 keeps the syntax colors.
    pub fg: u8,
    pub bg: u8,
    /// Gutter mark and its color; ' ' for none.
    pub mark: (char, u8),
}

/// Lines `a..=b` that can be shown as one row.
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    pub a: usize,
    pub b: usize,
    pub open: bool,
    pub label: String,
    /// Closed, it hides the rows above its lines too (a hunk and its deleted lines).
    pub hides_above: bool,
}

#[derive(Clone, Default, Debug)]
pub struct Decor {
    /// Rows drawn above a line, sorted by line; `lines.len()` means after the last.
    pub above: Vec<(usize, Vec<Virt>)>,
    /// Background per line; 0 for none.
    pub bg: Vec<u8>,
    /// Gutter mark per line; ' ' for none.
    pub mark: Vec<(char, u8)>,
    /// Sorted and not overlapping.
    pub folds: Vec<Fold>,
    /// Lines where changes start, for `]c` and `[c`.
    pub changes: Vec<usize>,
}

/// One screen row of a decorated buffer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Row {
    Line(usize),
    /// The `k`th row above line `l`.
    Virt(usize, usize),
    /// A closed fold, by index.
    Fold(usize),
}

impl Decor {
    pub fn above(&self, l: usize) -> &[Virt] {
        match self.above.binary_search_by_key(&l, |e| e.0) {
            Ok(i) => &self.above[i].1,
            Err(_) => &[],
        }
    }

    /// The fold whose lines include `l`, open or not.
    pub fn fold_at(&self, l: usize) -> Option<usize> {
        let i = self.folds.partition_point(|f| f.b < l);
        self.folds.get(i).filter(|f| f.a <= l).map(|_| i)
    }

    /// The closed fold hiding line `l`.
    pub fn closed(&self, l: usize) -> Option<usize> {
        self.fold_at(l).filter(|&i| !self.folds[i].open)
    }

    /// Where a cursor on line `l` shows: its closed fold's first line, else `l`.
    pub fn visible(&self, l: usize) -> usize {
        self.closed(l).map_or(l, |i| self.folds[i].a)
    }

    /// The next line that shows after `l`'s row, in a buffer of `n` lines.
    pub fn next(&self, l: usize, n: usize) -> Option<usize> {
        let end = self.closed(l).map_or(l, |i| self.folds[i].b);
        (end + 1 < n).then_some(end + 1)
    }

    /// The line that shows on the row before `l`'s.
    pub fn prev(&self, l: usize) -> Option<usize> {
        let start = self.visible(l);
        (start > 0).then(|| self.visible(start - 1))
    }

    /// Up to `max` rows from line `top`: each line's rows above it, then the
    /// line, or for a closed fold one row in place of all its lines.
    pub fn rows(&self, n: usize, top: usize, max: usize) -> Vec<Row> {
        let mut out = Vec::new();
        let mut l = self.visible(top.min(n.saturating_sub(1)));
        while out.len() < max && l <= n {
            let fold = if l < n { self.closed(l) } else { None };
            if !fold.is_some_and(|i| self.folds[i].hides_above) {
                for k in 0..self.above(l).len() {
                    if out.len() == max {
                        return out;
                    }
                    out.push(Row::Virt(l, k));
                }
            }
            if l == n || out.len() == max {
                break;
            }
            match fold {
                Some(i) => {
                    out.push(Row::Fold(i));
                    l = self.folds[i].b + 1;
                }
                None => {
                    out.push(Row::Line(l));
                    l += 1;
                }
            }
        }
        out
    }
}

/// Up to `max` rows of `b` from line `top`.
pub fn rows(b: &Buffer, top: usize, max: usize) -> Vec<Row> {
    match &b.decor {
        Some(d) => d.rows(b.lines.len(), top, max),
        None => (top..b.lines.len().min(top + max)).map(Row::Line).collect(),
    }
}

/// The index in `rows` of the row line `l` shows on.
pub fn row_of(b: &Buffer, rows: &[Row], l: usize) -> Option<usize> {
    let v = b.decor.as_ref().map_or(l, |d| d.visible(l));
    rows.iter().position(|r| match *r {
        Row::Line(x) => x == v,
        Row::Fold(i) => b.decor.as_ref().is_some_and(|d| d.folds[i].a == v),
        Row::Virt(..) => false,
    })
}

/// The lines (and closed folds, by their first line) among `rows`.
pub fn lines_of(b: &Buffer, rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .filter_map(|r| match *r {
            Row::Line(l) => Some(l),
            Row::Fold(i) => b.decor.as_ref().map(|d| d.folds[i].a),
            Row::Virt(..) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decor() -> Decor {
        let v = |t: &str| Virt { text: t.into(), ..Virt::default() };
        Decor {
            above: vec![(2, vec![v("gone 1"), v("gone 2")]), (10, vec![v("tail")])],
            folds: vec![Fold { a: 4, b: 7, open: false, label: "4 lines".into(), hides_above: false }],
            ..Decor::default()
        }
    }

    #[test]
    fn layout() {
        let d = decor();
        use Row::*;
        assert_eq!(d.rows(10, 0, 50), vec![Line(0), Line(1), Virt(2, 0), Virt(2, 1), Line(2), Line(3), Fold(0), Line(8), Line(9), Virt(10, 0)]);
        assert_eq!(d.rows(10, 5, 3), vec![Fold(0), Line(8), Line(9)]);
        assert_eq!(d.rows(10, 0, 3), vec![Line(0), Line(1), Virt(2, 0)]);
    }

    #[test]
    fn moving_over_folds() {
        let d = decor();
        assert_eq!(d.visible(6), 4);
        assert_eq!(d.next(3, 10), Some(4));
        assert_eq!(d.next(4, 10), Some(8));
        assert_eq!(d.prev(8), Some(4));
        assert_eq!(d.prev(4), Some(3));
        assert_eq!(d.next(9, 10), None);
    }
}
