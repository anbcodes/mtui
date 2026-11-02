// A course: its grading rules and your scores, read from and written to one
// text file (see the README for the format). All the arithmetic lives here.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Score {
    None,
    Got(f64, f64),
    /// A hypothetical score ("~90"): counts in projections, never in "so far".
    What(f64, f64),
}

pub fn num(x: f64) -> String {
    if x == x.round() && x.abs() < 1e12 {
        format!("{}", x as i64)
    } else {
        format!("{}", (x * 1e4).round() / 1e4)
    }
}

impl Score {
    pub fn parse(t: &str) -> Option<Score> {
        let t = t.trim();
        if matches!(t, "" | "-" | "?" | "_") {
            return Some(Score::None);
        }
        let (what, t) = match t.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, t),
        };
        let t = t.trim_end_matches('%');
        let (e, m) = match t.split_once('/') {
            Some((a, b)) => (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?),
            None => (t.parse::<f64>().ok()?, 100.0),
        };
        if !e.is_finite() || !m.is_finite() || m <= 0.0 {
            return None;
        }
        Some(if what { Score::What(e, m) } else { Score::Got(e, m) })
    }

    pub fn show(&self) -> String {
        let f = |e: f64, m: f64| if m == 100.0 { num(e) } else { format!("{}/{}", num(e), num(m)) };
        match *self {
            Score::None => "-".into(),
            Score::Got(e, m) => f(e, m),
            Score::What(e, m) => format!("~{}", f(e, m)),
        }
    }

    pub fn pct(&self) -> Option<f64> {
        match *self {
            Score::None => None,
            Score::Got(e, m) | Score::What(e, m) => Some(e / m * 100.0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String, // empty = a slot counted by `n=` that has no name yet
    pub score: Score,
    pub pre: Vec<String>,
    pub note: Option<String>,
}

impl Item {
    pub fn new(name: &str, score: Score) -> Item {
        Item { name: name.into(), score, pre: vec![], note: None }
    }
}

#[derive(Clone, Debug)]
pub struct Cat {
    pub name: String,
    pub weight: f64,
    pub fixed_n: bool,
    pub drop: usize,
    pub group: Option<String>,
    pub points: bool, // average by total points instead of the mean of percentages
    pub ranked: Vec<f64>, // weights by rank: the best item gets the first, and so on
    pub items: Vec<Item>,
    pub pre: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Require {
    pub group: String,
    pub min: f64,
    pub rounded: bool,
}

#[derive(Clone, Debug)]
pub struct Course {
    pub title: String,
    pub scale: Vec<(String, f64)>, // best first
    pub round: bool,
    pub requires: Vec<Require>,
    pub absence_rule: Option<(usize, usize)>, // free, letter steps per extra absence
    pub absent: usize,
    pub assume: Option<f64>,
    pub cats: Vec<Cat>,
    pub header: Vec<String>,
    pub tail: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Only scores you have: where you stand.
    Current,
    /// Every ungraded item scores this (hypothetical scores count as set).
    Proj(f64),
}

pub struct CatCalc {
    pub avg: Option<f64>,
    pub dropped: Vec<usize>,
}

pub struct ReqStatus {
    pub group: String,
    pub min: f64,
    pub value: Option<f64>,
    pub complete: bool,
}

impl ReqStatus {
    pub fn failed(&self) -> bool {
        self.value.is_some_and(|v| v < self.min)
    }
}

pub struct Eval {
    pub avg: Option<f64>,
    pub points: f64,
    pub covered: f64,
    pub rank: Option<usize>,
    pub cats: Vec<CatCalc>,
    pub reqs: Vec<ReqStatus>,
    pub penalty: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Need {
    Secured,
    Impossible,
    Avg(f64),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Head,
    Good,
    Warn,
    Bad,
    Dim,
    Plain,
}

impl Cat {
    pub fn calc(&self, mode: Mode) -> CatCalc {
        let known: Vec<f64> = self.items.iter().filter_map(|i| match i.score {
            Score::Got(_, m) | Score::What(_, m) => Some(m),
            Score::None => None,
        }).collect();
        let fm = if known.is_empty() { 100.0 } else { known.iter().sum::<f64>() / known.len() as f64 };
        let mut v: Vec<(usize, f64, f64)> = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            match (it.score, mode) {
                (Score::Got(e, m), _) => v.push((i, e, m)),
                (Score::What(e, m), Mode::Proj(_)) => v.push((i, e, m)),
                (Score::None, Mode::Proj(f)) => v.push((i, f / 100.0 * fm, fm)),
                _ => {}
            }
        }
        if v.is_empty() {
            return CatCalc { avg: None, dropped: vec![] };
        }
        // Drop the lowest only as far as the work that is in: with 3 of 10
        // homeworks in and one to drop, the drop is saved for later.
        let keep = self.items.len().saturating_sub(self.drop);
        let nd = v.len().saturating_sub(keep).min(v.len() - 1);
        let mut order: Vec<usize> = (0..v.len()).collect();
        order.sort_by(|&a, &b| (v[a].1 / v[a].2).partial_cmp(&(v[b].1 / v[b].2)).unwrap().then(a.cmp(&b)));
        let dropped: Vec<usize> = order[..nd].iter().map(|&k| v[k].0).collect();
        let kept: Vec<&(usize, f64, f64)> = v.iter().filter(|x| !dropped.contains(&x.0)).collect();
        let avg = if !self.ranked.is_empty() {
            let mut p: Vec<f64> = kept.iter().map(|x| x.1 / x.2 * 100.0).collect();
            p.sort_by(|a, b| b.partial_cmp(a).unwrap());
            let w = |k: usize| self.ranked.get(k).copied().unwrap_or(0.0);
            let tot: f64 = (0..p.len()).map(w).sum();
            if tot > 0.0 { p.iter().enumerate().map(|(k, x)| w(k) * x).sum::<f64>() / tot } else { p.iter().sum::<f64>() / p.len() as f64 }
        } else if self.points {
            kept.iter().map(|x| x.1).sum::<f64>() / kept.iter().map(|x| x.2).sum::<f64>() * 100.0
        } else {
            kept.iter().map(|x| x.1 / x.2 * 100.0).sum::<f64>() / kept.len() as f64
        };
        CatCalc { avg: Some(avg), dropped }
    }

    pub fn pending(&self) -> usize {
        self.items.iter().filter(|i| i.score == Score::None).count()
    }
}

impl Course {
    pub fn eval(&self, mode: Mode) -> Eval {
        let cats: Vec<CatCalc> = self.cats.iter().map(|c| c.calc(mode)).collect();
        let tw: f64 = self.cats.iter().map(|c| c.weight).sum();
        let (mut sw, mut sv) = (0.0, 0.0);
        for (c, k) in self.cats.iter().zip(&cats) {
            if let Some(a) = k.avg {
                sw += c.weight;
                sv += c.weight * a;
            }
        }
        let avg = (sw > 0.0).then(|| sv / sw);
        let reqs: Vec<ReqStatus> = self.requires.iter().map(|r| {
            let (mut gw, mut gv, mut complete) = (0.0, 0.0, true);
            for (c, k) in self.cats.iter().zip(&cats) {
                if c.group.as_deref() != Some(r.group.as_str()) {
                    continue;
                }
                if c.items.iter().any(|i| i.score == Score::None) && mode == Mode::Current {
                    complete = false;
                }
                if let Some(a) = k.avg {
                    gw += c.weight;
                    gv += c.weight * a;
                }
            }
            let value = (gw > 0.0).then(|| if r.rounded { (gv / gw).round() } else { gv / gw });
            ReqStatus { group: r.group.clone(), min: r.min, value, complete }
        }).collect();
        let penalty = self.absence_rule.map_or(0, |(free, step)| self.absent.saturating_sub(free) * step);
        let rank = avg.map(|a| {
            if reqs.iter().any(|r| r.complete && r.failed()) {
                self.scale.len()
            } else {
                (self.rank_of(a) + penalty).min(self.scale.len())
            }
        });
        Eval { avg, points: if tw > 0.0 { sv / tw } else { 0.0 }, covered: if tw > 0.0 { sw / tw } else { 0.0 }, rank, cats, reqs, penalty }
    }

    pub fn rank_of(&self, avg: f64) -> usize {
        let a = if self.round { avg.round() } else { avg };
        self.scale.iter().position(|(_, c)| a + 1e-9 >= *c).unwrap_or(self.scale.len())
    }

    pub fn letter(&self, rank: usize) -> String {
        self.scale.get(rank).map_or_else(|| "F".to_string(), |s| s.0.clone())
    }

    pub fn pending(&self) -> usize {
        self.cats.iter().map(|c| c.pending()).sum()
    }

    /// What ungraded work is assumed to score in the projection.
    pub fn fill(&self) -> f64 {
        self.assume.or_else(|| self.eval(Mode::Current).avg).unwrap_or(85.0)
    }

    pub fn need(&self, target: usize) -> Need {
        let ok = |x: f64| self.eval(Mode::Proj(x)).rank.is_some_and(|r| r <= target);
        if ok(0.0) {
            return Need::Secured;
        }
        if !ok(100.0) {
            return Need::Impossible;
        }
        let (mut lo, mut hi) = (0.0, 100.0);
        for _ in 0..50 {
            let mid = (lo + hi) / 2.0;
            if ok(mid) { hi = mid } else { lo = mid }
        }
        Need::Avg((hi * 10.0).ceil() / 10.0)
    }

    pub fn report(&self) -> Vec<(Tone, String)> {
        use Tone::*;
        let mut out = vec![];
        let cur = self.eval(Mode::Current);
        let Some(avg) = cur.avg else {
            out.push((Dim, "No scores yet.".to_string()));
            return out;
        };
        let rk = cur.rank.unwrap();
        out.push((Head, format!("So far     {:<3} {:.2}%", self.letter(rk), avg)));
        out.push((Dim, format!("           {:.0}% of the grade is in, {:.2} of 100 points", cur.covered * 100.0, cur.points)));
        let fill = self.fill();
        let p = self.eval(Mode::Proj(fill));
        let pr = p.rank.unwrap();
        let pending = self.pending();
        if pending > 0 {
            let src = if self.assume.is_some() { "assumed" } else { "your average so far" };
            out.push((Head, format!("Projected  {:<3} {:.2}%", self.letter(pr), p.avg.unwrap())));
            out.push((Dim, format!("           {} items left at {:.1}% ({})", pending, fill, src)));
            let hi = self.eval(Mode::Proj(100.0));
            let lo = self.eval(Mode::Proj(0.0));
            out.push((Plain, format!("Best case  {:<3} {:.2}%   all 100", self.letter(hi.rank.unwrap()), hi.avg.unwrap())));
            out.push((Plain, format!("Worst case {:<3} {:.2}%   all 0", self.letter(lo.rank.unwrap()), lo.avg.unwrap())));
            out.push((Plain, String::new()));
            out.push((Head, format!("Average needed on the {} items left", pending)));
            let mut shown = 0;
            let mut impossible: Option<usize> = None;
            let mut rows = vec![];
            for r in 0..self.scale.len() {
                match self.need(r) {
                    Need::Secured => break,
                    Need::Impossible => impossible = Some(r),
                    Need::Avg(x) => rows.push((r, x)),
                }
            }
            if let Some(r) = impossible {
                out.push((Dim, format!("  {:<3} out of reach", self.letter(r))));
            }
            for (r, x) in rows {
                if shown >= 8 {
                    break;
                }
                shown += 1;
                let t = if x <= fill { Good } else if x <= 90.0 { Warn } else { Bad };
                out.push((t, format!("  {:<3} {:>5.1}%", self.letter(r), x)));
            }
            if shown == 0 && impossible.is_none() {
                out.push((Good, format!("  already secured: {} or better", self.letter(0))));
            } else if let Some(r) = (0..self.scale.len()).find(|&r| self.need(r) == Need::Secured) {
                out.push((Good, format!("  {:<3} secured", self.letter(r))));
            }
        }
        let mut rules = vec![];
        for r in &p.reqs {
            let c = cur.reqs.iter().find(|c| c.group == r.group).unwrap();
            if let Some(v) = c.value {
                let t = if c.failed() { if c.complete { Bad } else { Warn } } else { Good };
                let note = if c.failed() { if c.complete { "  FAIL: course grade is F" } else { "  below the minimum so far" } } else { "" };
                rules.push((t, format!("  {} average {:.1} (need {}){}", r.group, v, num(r.min), note)));
            } else {
                rules.push((Dim, format!("  {} average: nothing graded yet (need {})", r.group, num(r.min))));
            }
        }
        if let Some((free, step)) = self.absence_rule {
            let t = if self.absent > free { Bad } else { Plain };
            let extra = if self.absent > free { format!(", -{} letter{}", cur.penalty, if cur.penalty == 1 { "" } else { "s" }) } else { String::new() };
            rules.push((t, format!("  absences {} ({} free, {} per extra){}", self.absent, free, step, extra)));
        }
        if !rules.is_empty() {
            out.push((Plain, String::new()));
            out.push((Head, "Rules".to_string()));
            out.extend(rules);
        }
        out
    }
}

// ---- file format ----

fn split_comment(l: &str) -> (&str, Option<String>) {
    let b = l.as_bytes();
    for (i, _) in l.match_indices('#') {
        if (i == 0 || b[i - 1].is_ascii_whitespace()) && (i + 1 >= b.len() || b[i + 1].is_ascii_whitespace()) {
            return (&l[..i], Some(l[i + 1..].trim().to_string()));
        }
    }
    (l, None)
}

fn kv(tok: &str) -> Option<(&str, &str)> {
    tok.split_once('=')
}

pub fn parse(src: &str) -> Result<Course, String> {
    let mut c = Course {
        title: String::new(),
        scale: vec![],
        round: false,
        requires: vec![],
        absence_rule: None,
        absent: 0,
        assume: None,
        cats: vec![],
        header: vec![],
        tail: vec![],
        warnings: vec![],
    };
    let mut pending: Vec<String> = vec![];
    let mut declared_n: Vec<usize> = vec![];
    for (ln, raw) in src.lines().enumerate() {
        let err = |m: &str| format!("line {}: {}", ln + 1, m);
        let (body, note) = split_comment(raw);
        let t = body.trim();
        if t.is_empty() {
            pending.push(match note {
                Some(n) => format!("# {}", n).trim_end().to_string(),
                None => String::new(),
            });
            continue;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        let indented = body.starts_with(char::is_whitespace);
        let kw = if indented { "" } else { toks[0] };
        let rest = &toks[1..];
        match kw {
            "course" => {
                c.title = rest.join(" ");
                c.header.append(&mut pending);
            }
            "scale" => {
                let flat = rest.join(" ").replace(',', " ");
                let t: Vec<&str> = flat.split_whitespace().collect();
                if t.is_empty() || t.len() % 2 != 0 {
                    return Err(err("scale wants pairs like `A 93, A- 90`"));
                }
                for p in t.chunks(2) {
                    let v: f64 = p[1].parse().map_err(|_| err(&format!("bad cutoff {}", p[1])))?;
                    c.scale.push((p[0].to_string(), v));
                }
                c.scale.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                c.header.append(&mut pending);
            }
            "round" => {
                c.round = matches!(rest.first().copied(), Some("yes" | "on" | "true"));
                c.header.append(&mut pending);
            }
            "require" => {
                // require GROUP >= 60 [rounded]
                if rest.len() < 3 || rest[1] != ">=" {
                    return Err(err("use `require GROUP >= 60 [rounded]`"));
                }
                let min: f64 = rest[2].parse().map_err(|_| err("bad minimum"))?;
                c.requires.push(Require { group: rest[0].to_string(), min, rounded: rest.get(3) == Some(&"rounded") });
                c.header.append(&mut pending);
            }
            "absence-penalty" => {
                let (mut free, mut step) = (0, 1);
                for t in rest {
                    match kv(t) {
                        Some(("free", v)) => free = v.parse().map_err(|_| err("bad free="))?,
                        Some(("step", v)) => step = v.parse().map_err(|_| err("bad step="))?,
                        _ => return Err(err("use `absence-penalty free=3 step=1`")),
                    }
                }
                c.absence_rule = Some((free, step));
                c.header.append(&mut pending);
            }
            "absent" | "absences" => {
                c.absent = rest.first().and_then(|v| v.parse().ok()).ok_or_else(|| err("absent wants a number"))?;
                c.header.append(&mut pending);
            }
            "assume" => {
                c.assume = rest.first().and_then(|v| v.trim_end_matches('%').parse().ok());
                if c.assume.is_none() {
                    return Err(err("assume wants a number"));
                }
                c.header.append(&mut pending);
            }
            "category" => {
                let wi = rest.iter().position(|t| t.ends_with('%')).ok_or_else(|| err("category NAME WEIGHT% — the weight needs a % sign"))?;
                let weight: f64 = rest[wi].trim_end_matches('%').parse().map_err(|_| err("bad weight"))?;
                let mut cat = Cat { name: rest[..wi].join(" "), weight, fixed_n: false, drop: 0, group: None, points: false, ranked: vec![], items: vec![], pre: std::mem::take(&mut pending), note };
                let mut n = 0;
                for t in &rest[wi + 1..] {
                    match kv(t) {
                        Some(("n", v)) => {
                            n = v.parse().map_err(|_| err("bad n="))?;
                            cat.fixed_n = true;
                        }
                        Some(("drop", v)) => cat.drop = v.parse().map_err(|_| err("bad drop="))?,
                        Some(("group", v)) => cat.group = Some(v.to_string()),
                        Some(("ranked", v)) => {
                            cat.ranked = v.split(',').map(|x| x.parse::<f64>()).collect::<Result<_, _>>().map_err(|_| err("ranked= wants weights like 20,20,10"))?;
                        }
                        None if *t == "points" => cat.points = true,
                        _ => return Err(err(&format!("unknown option {}", t))),
                    }
                }
                if cat.name.is_empty() {
                    return Err(err("category needs a name"));
                }
                declared_n.push(n);
                c.cats.push(cat);
            }
            _ => {
                let Some(cat) = c.cats.last_mut() else {
                    return Err(err(&format!("`{}` is not a setting; items go under a category", toks[0])));
                };
                let (name, score) = match Score::parse(toks[toks.len() - 1]) {
                    Some(s) if toks.len() > 1 || !matches!(s, Score::None) => (toks[..toks.len() - 1].join(" "), s),
                    _ => (toks.join(" "), Score::None),
                };
                cat.items.push(Item { name, score, pre: std::mem::take(&mut pending), note });
            }
        }
    }
    c.tail = pending;
    while c.tail.last().is_some_and(|l| l.is_empty()) {
        c.tail.pop();
    }
    if c.cats.is_empty() {
        return Err("no categories: add `category Homework 30%`".into());
    }
    if c.scale.is_empty() {
        c.scale = [("A", 93.0), ("A-", 90.0), ("B+", 87.0), ("B", 83.0), ("B-", 80.0), ("C+", 77.0), ("C", 73.0), ("C-", 70.0), ("D", 60.0)].iter().map(|(a, b)| (a.to_string(), *b)).collect();
        c.warnings.push("no `scale` line: using a standard A 93 … D 60 scale".into());
    }
    for (cat, n) in c.cats.iter_mut().zip(declared_n) {
        while cat.items.len() < n {
            cat.items.push(Item::new("", Score::None));
        }
        if cat.drop >= cat.items.len().max(1) && cat.drop > 0 {
            c.warnings.push(format!("{}: drop={} but only {} items", cat.name, cat.drop, cat.items.len()));
        }
    }
    let tw: f64 = c.cats.iter().map(|k| k.weight).sum();
    if (tw - 100.0).abs() > 0.01 {
        c.warnings.push(format!("category weights add up to {}, not 100", num(tw)));
    }
    for r in &c.requires {
        if !c.cats.iter().any(|k| k.group.as_deref() == Some(r.group.as_str())) {
            c.warnings.push(format!("require {}: no category has group={}", r.group, r.group));
        }
    }
    Ok(c)
}

fn push_pre(out: &mut String, pre: &[String]) {
    for l in pre {
        out.push_str(l);
        out.push('\n');
    }
}

fn push_note(out: &mut String, note: &Option<String>) {
    if let Some(n) = note {
        out.push_str("  # ");
        out.push_str(n);
    }
}

impl Course {
    pub fn serialize(&self) -> String {
        let mut o = String::new();
        push_pre(&mut o, &self.header);
        if !self.title.is_empty() {
            o.push_str(&format!("course {}\n", self.title));
        }
        let sc: Vec<String> = self.scale.iter().map(|(l, v)| format!("{} {}", l, num(*v))).collect();
        o.push_str(&format!("scale {}\n", sc.join(", ")));
        if self.round {
            o.push_str("round yes\n");
        }
        for r in &self.requires {
            o.push_str(&format!("require {} >= {}{}\n", r.group, num(r.min), if r.rounded { " rounded" } else { "" }));
        }
        if let Some((free, step)) = self.absence_rule {
            o.push_str(&format!("absence-penalty free={} step={}\n", free, step));
        }
        if self.absence_rule.is_some() || self.absent > 0 {
            o.push_str(&format!("absent {}\n", self.absent));
        }
        if let Some(a) = self.assume {
            o.push_str(&format!("assume {}\n", num(a)));
        }
        for (i, c) in self.cats.iter().enumerate() {
            if i == 0 && c.pre.first().is_none_or(|l| !l.is_empty()) {
                o.push('\n');
            }
            push_pre(&mut o, &c.pre);
            o.push_str(&format!("category {} {}%", c.name, num(c.weight)));
            if c.fixed_n {
                o.push_str(&format!(" n={}", c.items.len()));
            }
            if c.drop > 0 {
                o.push_str(&format!(" drop={}", c.drop));
            }
            if let Some(g) = &c.group {
                o.push_str(&format!(" group={}", g));
            }
            if c.points {
                o.push_str(" points");
            }
            if !c.ranked.is_empty() {
                o.push_str(&format!(" ranked={}", c.ranked.iter().map(|w| num(*w)).collect::<Vec<_>>().join(",")));
            }
            push_note(&mut o, &c.note);
            o.push('\n');
            for it in &c.items {
                if it.name.is_empty() && it.score == Score::None {
                    continue;
                }
                push_pre(&mut o, &it.pre);
                o.push_str(&format!("  {} {}", it.name, it.score.show()));
                push_note(&mut o, &it.note);
                o.push('\n');
            }
        }
        if !self.tail.is_empty() {
            o.push('\n');
            push_pre(&mut o, &self.tail);
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
# my course
course Databases
scale A 93, A- 90, B+ 87, B 83, B- 80, C+ 77, C 73, C- 70, D+ 67, D 60
require exams >= 60 rounded

category Exercises 30% n=4
  #1 100
  #2 90
category Homework 26% n=4 drop=1
  HW1 80
  HW2 100
  HW3 90   # late
category Midterm 1 14% group=exams
  Exam 70
category Midterm 2 14% group=exams
  Exam -
category Final 16% group=exams
  Exam -
";

    #[test]
    fn roundtrip_keeps_comments() {
        let c = parse(SRC).unwrap();
        assert_eq!(c.title, "Databases");
        assert_eq!(c.cats.len(), 5);
        assert_eq!(c.cats[0].items.len(), 4);
        let s = c.serialize();
        let c2 = parse(&s).unwrap();
        assert_eq!(c2.serialize(), s);
        assert!(s.contains("# my course") && s.contains("# late"));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn drop_is_saved_for_later() {
        let c = parse(SRC).unwrap();
        // homework: 3 of 4 in, drop 1 -> nothing dropped yet
        let k = c.cats[1].calc(Mode::Current);
        assert!(k.dropped.is_empty());
        assert!((k.avg.unwrap() - 90.0).abs() < 1e-9);
        // projected at 50 for HW4: lowest (50) is dropped
        let k = c.cats[1].calc(Mode::Proj(50.0));
        assert_eq!(k.dropped, vec![3]);
        assert!((k.avg.unwrap() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn standing_and_projection() {
        let c = parse(SRC).unwrap();
        let e = c.eval(Mode::Current);
        // exercises 95 *30 + hw 90 *26 + midterm 70 *14 over 70
        let want = (95.0 * 30.0 + 90.0 * 26.0 + 70.0 * 14.0) / 70.0;
        assert!((e.avg.unwrap() - want).abs() < 1e-9);
        assert_eq!(c.letter(e.rank.unwrap()), "B+");
        assert!(!e.reqs[0].complete);
        let p = c.eval(Mode::Proj(100.0));
        assert_eq!(c.letter(p.rank.unwrap()), "A"); // 95.8
    }

    #[test]
    fn exam_rule_forces_f() {
        let c = parse(SRC).unwrap();
        // everything else perfect but exams average 20
        let mut c = c;
        c.cats[2].items[0].score = Score::Got(20.0, 100.0);
        c.cats[3].items[0].score = Score::Got(20.0, 100.0);
        c.cats[4].items[0].score = Score::Got(20.0, 100.0);
        for k in &mut c.cats[..2] {
            for i in &mut k.items {
                i.score = Score::Got(100.0, 100.0);
            }
        }
        let e = c.eval(Mode::Current);
        assert_eq!(c.letter(e.rank.unwrap()), "F");
    }

    #[test]
    fn needed_average() {
        let c = parse(SRC).unwrap();
        match c.need(2) {
            Need::Avg(x) => {
                let e = c.eval(Mode::Proj(x));
                assert_eq!(c.letter(e.rank.unwrap()), "B+");
                let e = c.eval(Mode::Proj(x - 0.2));
                assert_ne!(c.letter(e.rank.unwrap()), "B+");
            }
            n => panic!("{:?}", n),
        }
        // even a D needs passing exams: zeros on the rest of the exams would fail the rule
        assert!(matches!(c.need(9), Need::Avg(x) if x > 50.0 && x < 60.0));
        assert_eq!(c.need(10), Need::Secured);
    }

    #[test]
    fn absences_and_points_and_whatif() {
        let c = parse("scale A 93, A- 89, B+ 86\nabsence-penalty free=3 step=1\nabsent 4\ncategory Papers 100% points\n  P1 40/50\n  P2 ~50/50\n").unwrap();
        let cur = c.eval(Mode::Current);
        assert!((cur.avg.unwrap() - 80.0).abs() < 1e-9);
        let p = c.eval(Mode::Proj(0.0));
        assert!((p.avg.unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(p.penalty, 1);
        assert_eq!(c.letter(p.rank.unwrap()), "B+"); // A- minus one
        assert_eq!(Score::parse("17/20").unwrap().show(), "17/20");
        assert_eq!(Score::parse("~85%").unwrap().show(), "~85");
        assert!(Score::parse("x").is_none());
    }

    #[test]
    fn ranked_weights() {
        let c = parse("scale A 90, B 80\ncategory PAs 50% ranked=20,20,10\n  PA1 60\n  PA2 100\n  PA3 80\ncategory Exams 50%\n  E 100\n").unwrap();
        // best two at 20 each, the lowest at 10: (20*100 + 20*80 + 10*60) / 50
        let k = c.cats[0].calc(Mode::Current);
        assert!((k.avg.unwrap() - 84.0).abs() < 1e-9);
        // with only two in, they take the top weights
        let c = parse("scale A 90\ncategory PAs 50% ranked=20,20,10 n=3\n  PA1 60\n  PA2 100\n").unwrap();
        assert!((c.cats[0].calc(Mode::Current).avg.unwrap() - 80.0).abs() < 1e-9);
        // the missing one at 100 is the best, so the 60 falls to the 10 weight
        assert!((c.cats[0].calc(Mode::Proj(100.0)).avg.unwrap() - 92.0).abs() < 1e-9);
        assert!(c.serialize().contains("ranked=20,20,10"));
    }

    #[test]
    fn errors() {
        assert!(parse("HW1 90\n").is_err());
        assert!(parse("category Homework 30\n").is_err());
        assert!(parse("").is_err());
    }
}
