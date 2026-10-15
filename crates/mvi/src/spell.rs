// Spelling and grammar for prose (markdown and text buffers).
//
// Spelling reads a Hunspell dictionary (the system's en_US unless
// `spelllang` says otherwise) and looks words up the way Hunspell does: the
// stem table plus prefix/suffix rules, applied in reverse. The table points
// into the .dic text itself, so en_US costs about 1.4 MB and loads in a few
// milliseconds. Grammar is a short list of high-precision rules (doubled
// words, a/an, sentence capitals, spacing around punctuation and well-known
// confusions); it stays quiet wherever a careful editor might hesitate.
//
// Results are cached per line text, so after the first pass a keystroke
// rechecks one line.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

fn hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[derive(Default, Clone, Copy)]
pub struct Fnv(u64);

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, b: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fnv>>;

/// A line's cache key: eight bytes at a time, since every line is hashed
/// on every edit.
fn line_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15 ^ s.len() as u64;
    let mut it = s.chunks_exact(8);
    for c in &mut it {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x5851_f42d_4c95_7f2d).rotate_left(29);
    }
    for &b in it.remainder() {
        h = (h ^ b as u64).wrapping_mul(0x5851_f42d_4c95_7f2d).rotate_left(29);
    }
    h ^ (h >> 32)
}

// ---------- dictionary ----------

#[derive(Clone, Copy, PartialEq)]
enum FlagMode {
    Char,
    Long,
    Num,
}

#[derive(Debug)]
enum Cond {
    Any,
    Set(Vec<char>, bool), // chars, negated
}

impl Cond {
    fn ok(&self, c: char) -> bool {
        match self {
            Cond::Any => true,
            Cond::Set(cs, neg) => cs.contains(&c) != *neg,
        }
    }
}

fn parse_cond(s: &str) -> Vec<Cond> {
    if s == "." {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            '.' => out.push(Cond::Any),
            '[' => {
                let mut set = Vec::new();
                let mut neg = false;
                let mut first = true;
                for c in it.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if first && c == '^' {
                        neg = true;
                    } else {
                        set.push(c);
                    }
                    first = false;
                }
                out.push(Cond::Set(set, neg));
            }
            c => out.push(Cond::Set(vec![c], false)),
        }
    }
    out
}

struct Affix {
    flag: String,
    cross: bool,
    strip: String,
    add: String,
    cond: Vec<Cond>,
}

impl Affix {
    /// The condition holds at the end (suffix) or start (prefix) of `root`.
    fn holds(&self, root: &str, suffix: bool) -> bool {
        let n = self.cond.len();
        if n == 0 {
            return true;
        }
        if suffix {
            let mut cs = root.chars().rev();
            self.cond.iter().rev().all(|c| cs.next().map_or(false, |ch| c.ok(ch)))
        } else {
            let mut cs = root.chars();
            self.cond.iter().all(|c| cs.next().map_or(false, |ch| c.ok(ch)))
        }
    }
}

pub struct Dict {
    text: String,    // the .dic contents; each entry is a line of it
    table: Vec<u32>, // open addressing: line start + 1, 0 = empty
    mode: FlagMode,
    aliases: Vec<String>,
    pfx: Vec<Affix>,
    sfx: Vec<Affix>,
    sfx_by_end: HashMap<u8, Vec<usize>>, // suffix rules by the last byte they add (0: add nothing)
    nosuggest: String,
    forbidden: String,
    onlyincompound: String,
    needaffix: String,
    try_chars: Vec<char>,
    reps: Vec<(String, String)>,
}

/// Bytes in the dictionary's declared encoding as a String.
fn decode(bytes: Vec<u8>, utf8: bool) -> String {
    if utf8 {
        String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

fn entry_word(line: &str) -> &str {
    let end = line.find(|c: char| c == '/' || c == '\t' || c == ' ').unwrap_or(line.len());
    &line[..end]
}

fn entry_flags(line: &str) -> &str {
    match line.find('/') {
        Some(i) => {
            let f = &line[i + 1..];
            &f[..f.find(|c: char| c == '\t' || c == ' ').unwrap_or(f.len())]
        }
        None => "",
    }
}

impl Dict {
    /// Build from Hunspell .aff and .dic text (an empty `aff` reads `dic` as
    /// a plain word list such as /usr/share/dict/words).
    pub fn new(aff: &str, dic: String) -> Dict {
        let mut d = Dict {
            text: String::new(),
            table: Vec::new(),
            mode: FlagMode::Char,
            aliases: Vec::new(),
            pfx: Vec::new(),
            sfx: Vec::new(),
            sfx_by_end: HashMap::new(),
            nosuggest: String::new(),
            forbidden: String::new(),
            onlyincompound: String::new(),
            needaffix: String::new(),
            try_chars: Vec::new(),
            reps: Vec::new(),
        };
        let mut cross: HashMap<(bool, String), bool> = HashMap::new();
        for l in aff.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            match f.as_slice() {
                ["FLAG", m, ..] => {
                    d.mode = match *m {
                        "long" => FlagMode::Long,
                        "num" => FlagMode::Num,
                        _ => FlagMode::Char,
                    }
                }
                ["AF", a, ..] if a.parse::<usize>().is_err() => d.aliases.push(a.to_string()),
                ["NOSUGGEST", f, ..] => d.nosuggest = f.to_string(),
                ["FORBIDDENWORD", f, ..] => d.forbidden = f.to_string(),
                ["ONLYINCOMPOUND", f, ..] => d.onlyincompound = f.to_string(),
                ["NEEDAFFIX", f, ..] | ["PSEUDOROOT", f, ..] => d.needaffix = f.to_string(),
                ["TRY", t, ..] => d.try_chars = t.chars().filter(|c| c.is_lowercase() || *c == '\'' || *c == '-').collect(),
                ["REP", a, b, ..] if a.parse::<usize>().is_err() => d.reps.push((a.replace('_', " "), b.replace('_', " "))),
                [kind @ ("PFX" | "SFX"), flag, c, n] if n.parse::<usize>().is_ok() && (*c == "Y" || *c == "N") => {
                    cross.insert((*kind == "SFX", flag.to_string()), *c == "Y");
                }
                [kind @ ("PFX" | "SFX"), flag, strip, add, rest @ ..] => {
                    let sfx = *kind == "SFX";
                    let add = add.split('/').next().unwrap_or("");
                    let a = Affix {
                        flag: flag.to_string(),
                        cross: cross.get(&(sfx, flag.to_string())).copied().unwrap_or(false),
                        strip: if *strip == "0" { String::new() } else { strip.to_string() },
                        add: if add == "0" { String::new() } else { add.to_string() },
                        cond: parse_cond(rest.first().copied().unwrap_or(".")),
                    };
                    if sfx {
                        d.sfx.push(a);
                    } else {
                        d.pfx.push(a);
                    }
                }
                _ => {}
            }
        }
        for (i, a) in d.sfx.iter().enumerate() {
            d.sfx_by_end.entry(a.add.bytes().last().unwrap_or(0)).or_default().push(i);
        }
        if d.try_chars.is_empty() {
            d.try_chars = "esiarntolcdugmphbyfvkwzxjq'".chars().collect();
        }
        // the table, pointing at line starts
        d.text = dic;
        let n = d.text.lines().count();
        let cap = (n * 2).next_power_of_two().max(16);
        let mut table = vec![0u32; cap];
        let mask = cap - 1;
        let mut off = 0usize;
        let mut first = true;
        for line in d.text.split('\n') {
            let start = off;
            off += line.len() + 1;
            let line = line.trim_end_matches('\r');
            if first {
                first = false;
                if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) {
                    continue; // the entry count
                }
            }
            let w = entry_word(line);
            if w.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut i = hash(w.as_bytes()) as usize & mask;
            while table[i] != 0 {
                i = (i + 1) & mask;
            }
            table[i] = start as u32 + 1;
        }
        d.table = table;
        d
    }

    /// Call `f` with the flags of every entry spelled `w`; stops at the
    /// first `true`.
    fn find(&self, w: &str, mut f: impl FnMut(&str) -> bool) -> bool {
        let mask = self.table.len() - 1;
        let mut i = hash(w.as_bytes()) as usize & mask;
        loop {
            let e = self.table[i];
            if e == 0 {
                return false;
            }
            let rest = &self.text[e as usize - 1..];
            let line = rest[..rest.find('\n').unwrap_or(rest.len())].trim_end_matches('\r');
            if entry_word(line) == w {
                let mut flags = entry_flags(line);
                if !self.aliases.is_empty() {
                    if let Ok(n) = flags.parse::<usize>() {
                        flags = self.aliases.get(n.wrapping_sub(1)).map_or("", |s| s.as_str());
                    }
                }
                if f(flags) {
                    return true;
                }
            }
            i = (i + 1) & mask;
        }
    }

    fn has(&self, flags: &str, flag: &str) -> bool {
        if flag.is_empty() || flags.is_empty() {
            return false;
        }
        match self.mode {
            FlagMode::Char => flags.contains(flag),
            FlagMode::Num => flags.split(',').any(|f| f == flag),
            FlagMode::Long => {
                let cs: Vec<char> = flags.chars().collect();
                cs.chunks(2).any(|c| c.iter().collect::<String>() == flag)
            }
        }
    }

    fn plain_ok(&self, flags: &str) -> bool {
        !self.has(flags, &self.forbidden) && !self.has(flags, &self.onlyincompound) && !self.has(flags, &self.needaffix)
    }

    /// `w` is a word as written (case included): a stem, or a stem with one
    /// suffix and/or one prefix.
    pub fn word_ok(&self, w: &str) -> bool {
        if self.find(w, |f| self.plain_ok(f)) {
            return true;
        }
        let none = Vec::new();
        let last = w.bytes().last().unwrap_or(0);
        let cands = self.sfx_by_end.get(&last).unwrap_or(&none).iter().chain(self.sfx_by_end.get(&0).unwrap_or(&none));
        for a in cands.map(|&i| &self.sfx[i]) {
            if !w.ends_with(&a.add) || w.len() <= a.add.len() && a.strip.is_empty() {
                continue;
            }
            let root = format!("{}{}", &w[..w.len() - a.add.len()], a.strip);
            if root.is_empty() || !a.holds(&root, true) {
                continue;
            }
            if self.find(&root, |f| self.has(f, &a.flag) && !self.has(f, &self.forbidden)) {
                return true;
            }
            if a.cross {
                for p in self.pfx.iter().filter(|p| p.cross) {
                    if !root.starts_with(&p.add) || root.len() <= p.add.len() && p.strip.is_empty() {
                        continue;
                    }
                    let r2 = format!("{}{}", p.strip, &root[p.add.len()..]);
                    if p.holds(&r2, false) && self.find(&r2, |f| self.has(f, &a.flag) && self.has(f, &p.flag)) {
                        return true;
                    }
                }
            }
        }
        for p in &self.pfx {
            if !w.starts_with(&p.add) || w.len() <= p.add.len() && p.strip.is_empty() {
                continue;
            }
            let root = format!("{}{}", p.strip, &w[p.add.len()..]);
            if p.holds(&root, false) && self.find(&root, |f| self.has(f, &p.flag) && !self.has(f, &self.forbidden)) {
                return true;
            }
        }
        false
    }

    /// Like `word_ok`, allowing for capitals the way prose uses them: a
    /// capitalised or ALL-CAPS word matches the dictionary's lower-case one.
    pub fn check(&self, w: &str) -> bool {
        if self.word_ok(w) {
            return true;
        }
        match case_of(w) {
            Case::Title => self.word_ok(&w.to_lowercase()),
            Case::Upper => {
                let l = w.to_lowercase();
                self.word_ok(&l) || self.word_ok(&title(&l))
            }
            _ => false,
        }
    }

    /// Every form of the entry on `line`: the stem and the stem with each
    /// prefix/suffix its flags allow (none of NOSUGGEST entries).
    fn forms(&self, line: &str, out: &mut Vec<String>) {
        let stem = entry_word(line);
        let mut flags = entry_flags(line);
        if !self.aliases.is_empty() {
            if let Ok(n) = flags.parse::<usize>() {
                flags = self.aliases.get(n.wrapping_sub(1)).map_or("", |s| s.as_str());
            }
        }
        if self.has(flags, &self.nosuggest) || self.has(flags, &self.forbidden) {
            return;
        }
        if self.plain_ok(flags) {
            out.push(stem.to_string());
        }
        let pfxs: Vec<&Affix> = self.pfx.iter().filter(|p| self.has(flags, &p.flag) && stem.starts_with(&p.strip) && p.holds(stem, false)).collect();
        for p in &pfxs {
            out.push(format!("{}{}", p.add, &stem[p.strip.len()..]));
        }
        for a in &self.sfx {
            if self.has(flags, &a.flag) && stem.ends_with(&a.strip) && a.holds(stem, true) {
                let f = format!("{}{}", &stem[..stem.len() - a.strip.len()], a.add);
                if a.cross {
                    for p in pfxs.iter().filter(|p| p.cross) {
                        out.push(format!("{}{}", p.add, &f[p.strip.len().min(f.len())..]));
                    }
                }
                out.push(f);
            }
        }
    }

    /// Dictionary words that share the most letter pairs with `w` (lower
    /// case), with their affixed forms: the suggestions edits can't reach.
    fn near_words(&self, w: &str, keep: usize) -> Vec<String> {
        let grams: Vec<[u8; 2]> = w.as_bytes().windows(2).map(|p| [p[0], p[1]]).collect();
        if grams.is_empty() {
            return Vec::new();
        }
        let mut top: Vec<(f32, &str)> = Vec::with_capacity(keep + 1);
        let mut floor = 0.0f32;
        for line in self.text.split('\n') {
            let sb = entry_word(line).as_bytes();
            if sb.len() < 2 || sb.len() + 6 < w.len() || sb.len() > w.len() + 4 {
                continue;
            }
            let mut common = 0;
            for p in sb.windows(2) {
                if grams.contains(&[p[0].to_ascii_lowercase(), p[1].to_ascii_lowercase()]) {
                    common += 1;
                }
            }
            let score = 2.0 * common as f32 / (grams.len() + sb.len() - 1) as f32;
            if score > floor || top.len() < keep {
                let at = top.partition_point(|x| x.0 >= score);
                top.insert(at, (score, line));
                if top.len() > keep {
                    top.pop();
                }
                if top.len() == keep {
                    floor = top[keep - 1].0;
                }
            }
        }
        let mut out = Vec::new();
        for (_, line) in top {
            self.forms(line.trim_end_matches('\r'), &mut out);
        }
        out
    }

    fn suggestible(&self, w: &str) -> bool {
        self.check(w) && !(!self.nosuggest.is_empty() && self.find(w, |f| self.has(f, &self.nosuggest)))
    }
}

fn dict_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push(h.join(".config/mvi/spell"));
        v.push(h.join(".local/share/hunspell"));
        v.push(h.join("Library/Spelling"));
    }
    if let Some(p) = std::env::var_os("DICPATH") {
        v.extend(std::env::split_paths(&p));
    }
    for d in ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts", "/usr/local/share/hunspell", "/opt/homebrew/share/hunspell", "/Library/Spelling"] {
        v.push(PathBuf::from(d));
    }
    v
}

/// Load the dictionary for `lang` (en_US, en_GB, de_DE, ...).
pub fn load(lang: &str) -> Result<Dict, String> {
    let names = [lang.to_string(), lang.replace('-', "_"), lang.split(['_', '-']).next().unwrap_or(lang).to_string()];
    for dir in dict_dirs() {
        for n in &names {
            let (a, d) = (dir.join(format!("{}.aff", n)), dir.join(format!("{}.dic", n)));
            if let (Ok(aff), Ok(dic)) = (std::fs::read(&a), std::fs::read(&d)) {
                let head = String::from_utf8_lossy(&aff[..aff.len().min(4096)]).to_string();
                let set = head.lines().find_map(|l| l.strip_prefix("SET ")).unwrap_or("UTF-8").trim().to_ascii_uppercase();
                let utf8 = set == "UTF-8" || set == "UTF8";
                return Ok(Dict::new(&decode(aff, utf8), decode(dic, utf8)));
            }
        }
    }
    if lang.starts_with("en") {
        for p in ["/usr/share/dict/words", "/usr/share/dict/american-english", "/usr/share/dict/british-english"] {
            if let Ok(b) = std::fs::read(p) {
                return Ok(Dict::new("", decode(b, true)));
            }
        }
    }
    Err(format!("no {} dictionary (install hunspell-{}, or put {}.aff/.dic in ~/.config/mvi/spell)", lang, lang.to_lowercase().replace('_', "-"), lang))
}

pub fn default_lang() -> String {
    for v in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(s) = std::env::var(v) {
            let s = s.split('.').next().unwrap_or("");
            if s.len() >= 2 && s != "C" && s != "POSIX" {
                return s.to_string();
            }
        }
    }
    "en_US".into()
}

// ---------- case ----------

#[derive(PartialEq, Clone, Copy)]
enum Case {
    Lower,
    Title,
    Upper,
    Mixed,
}

fn case_of(w: &str) -> Case {
    let mut cs = w.chars().filter(|c| c.is_alphabetic());
    let Some(f) = cs.next() else { return Case::Lower };
    let (mut up, mut low) = (0, 0);
    for c in cs {
        if c.is_uppercase() {
            up += 1;
        } else {
            low += 1;
        }
    }
    match (f.is_uppercase(), up, low) {
        (false, 0, _) => Case::Lower,
        (true, 0, _) => Case::Title,
        (true, _, 0) => Case::Upper,
        _ => Case::Mixed,
    }
}

fn title(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn like(case: Case, w: &str) -> String {
    match case {
        Case::Title => title(w),
        Case::Upper => w.to_uppercase(),
        _ => w.to_string(),
    }
}

// ---------- checking ----------

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub s: usize, // byte range in the line
    pub e: usize,
    pub grammar: bool,
    pub msg: String,
    pub fixes: Vec<String>,
}

pub struct Speller {
    pub lang: String,
    dict: Option<Result<Dict, String>>,
    user: HashSet<String>,
    ignore: HashSet<String>,
    cache: FastMap<u64, Rc<[Hit]>>,
    words: std::cell::RefCell<FastMap<Box<str>, bool>>, // verdicts so far
    pub grammar: bool,
    pub strict: bool, // English: only the dictionary's own spelling (no colour/color)
    pub gen: u64,     // bumps whenever results could change
}

fn user_file() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mvi/words"))
}

impl Default for Speller {
    fn default() -> Speller {
        Speller { lang: default_lang(), dict: None, user: HashSet::new(), ignore: HashSet::new(), cache: Default::default(), words: Default::default(), grammar: true, strict: false, gen: 1 }
    }
}

impl Speller {
    /// Load the dictionary and personal word list the first time they're
    /// needed; returns the error to show if the dictionary is missing.
    pub fn ensure(&mut self) -> Option<String> {
        if self.dict.is_none() {
            self.dict = Some(load(&self.lang));
            if let Some(Ok(s)) = user_file().map(std::fs::read_to_string) {
                self.user = s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            }
            self.changed();
            if let Some(Err(e)) = &self.dict {
                return Some(e.clone());
            }
        }
        None
    }

    pub fn set_lang(&mut self, lang: &str) {
        self.lang = lang.to_string();
        self.dict = None;
        self.changed();
    }

    pub fn set_grammar(&mut self, on: bool) {
        self.grammar = on;
        self.changed();
    }

    pub fn set_strict(&mut self, on: bool) {
        self.strict = on;
        self.changed();
    }

    fn changed(&mut self) {
        self.cache.clear();
        self.words.borrow_mut().clear();
        self.gen += 1;
    }

    fn dict(&self) -> Option<&Dict> {
        self.dict.as_ref().and_then(|d| d.as_ref().ok())
    }

    /// Accept `w` from now on: in this session, and (`save`) in
    /// ~/.config/mvi/words.
    pub fn add(&mut self, w: &str, save: bool) -> Result<(), String> {
        let w = w.replace('’', "'");
        if save {
            let f = user_file().ok_or("no $HOME")?;
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            use std::io::Write;
            let mut fh = std::fs::OpenOptions::new().create(true).append(true).open(&f).map_err(|e| e.to_string())?;
            writeln!(fh, "{}", w).map_err(|e| e.to_string())?;
            self.user.insert(w);
        } else {
            self.ignore.insert(w);
        }
        self.changed();
        Ok(())
    }

    fn known(&self, w: &str) -> bool {
        let l = w.to_lowercase();
        for set in [&self.user, &self.ignore] {
            if set.contains(w) || set.contains(&l) {
                return true;
            }
        }
        if TECH.binary_search(&l.as_str()).is_ok() {
            return true;
        }
        let Some(d) = self.dict() else { return true };
        if d.check(w) {
            return true;
        }
        // English: either spelling of colour/color, organise/organize, ...
        !self.strict && self.lang.starts_with("en") && variants(&l, !self.lang.starts_with("en_US") && !self.lang.starts_with("en-US")).iter().any(|v| d.check(v))
    }

    fn word_good(&self, w: &str) -> bool {
        if let Some(&v) = self.words.borrow().get(w) {
            return v;
        }
        let v = self.judge(w);
        let mut m = self.words.borrow_mut();
        if m.len() > 100_000 {
            m.clear();
        }
        m.insert(w.into(), v);
        v
    }

    fn judge(&self, w: &str) -> bool {
        if self.known(w) {
            return true;
        }
        // possessives the dictionary has no rule for
        if let Some(b) = w.strip_suffix("'s").or_else(|| w.strip_suffix("'S")) {
            return !b.is_empty() && self.known(b);
        }
        false
    }

    /// Problems in a paragraph (its lines, joined by newlines in the hit
    /// offsets). Cached on the lines' text, so unchanged paragraphs cost a
    /// hash per line.
    pub fn block(&mut self, lines: &[String], md: bool) -> Rc<[Hit]> {
        if lines.len() == 1 {
            return self.line(&lines[0], md);
        }
        let mut key = md as u64;
        for l in lines {
            key = (key ^ line_hash(l.as_bytes())).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(17);
        }
        if let Some(h) = self.cache.get(&key) {
            return h.clone();
        }
        let h: Rc<[Hit]> = self.scan(&lines.join("\n"), md).into();
        self.cache.insert(key, h.clone());
        h
    }

    /// Problems in one line (or any text; `md`: markdown syntax is skipped).
    pub fn line(&mut self, line: &str, md: bool) -> Rc<[Hit]> {
        let key = line_hash(line.as_bytes()) ^ md as u64;
        if let Some(h) = self.cache.get(&key) {
            return h.clone();
        }
        if self.cache.len() > 200_000 {
            self.cache.clear();
        }
        let h: Rc<[Hit]> = self.scan(line, md).into();
        self.cache.insert(key, h.clone());
        h
    }

    fn scan(&self, line: &str, md: bool) -> Vec<Hit> {
        let mut out = Vec::new();
        if md && is_ref_definition(line) {
            return out;
        }
        let skip = skips(line, md);
        let toks = tokens(line, &skip);
        if self.dict().is_some() {
            for t in &toks {
                if t.code {
                    continue;
                }
                for (s, e) in parts(line, t) {
                    let w = &line[s..e];
                    if w.chars().count() < 2 || matches!(case_of(w), Case::Upper | Case::Mixed) {
                        continue;
                    }
                    let n: std::borrow::Cow<str> = if w.contains('’') { w.replace('’', "'").into() } else { w.into() };
                    if !self.word_good(&n) {
                        out.push(Hit { s, e, grammar: false, msg: String::new(), fixes: Vec::new() });
                    }
                }
            }
        }
        if self.grammar {
            let known = |w: &str| self.dict().is_none() || self.word_good(w);
            grammar(line, &toks, &skip, &known, &mut out);
        }
        out.sort_by_key(|h| (h.s, h.e));
        out
    }

    /// Replacements for a misspelt word, best first.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let Some(d) = self.dict() else { return Vec::new() };
        let case = case_of(word);
        let w = word.replace('’', "'").to_lowercase();
        let n = w.chars().count();
        let mut cost: HashMap<String, f32> = HashMap::new();
        let put = |c: String, k: f32, cost: &mut HashMap<String, f32>| {
            let e = cost.entry(c).or_insert(f32::MAX);
            if k < *e {
                *e = k;
            }
        };
        for (bad, good) in COMMON {
            if *bad == w {
                put(good.to_string(), 0.0, &mut cost);
            }
        }
        if d.suggestible(&title(&w)) && !d.word_ok(&w) {
            put(title(&w), 0.1, &mut cost);
        }
        for (a, b) in &d.reps {
            let mut from = 0;
            while let Some(i) = w[from..].find(a.as_str()) {
                let i = from + i;
                let c = format!("{}{}{}", &w[..i], b, &w[i + a.len()..]);
                if c.split(' ').all(|p| d.suggestible(p)) {
                    put(c, 0.5, &mut cost);
                }
                from = i + a.len().max(1);
            }
        }
        let e1 = edits(&w, &d.try_chars);
        for (c, k) in &e1 {
            if d.suggestible(c) {
                put(c.clone(), *k, &mut cost);
            }
        }
        // two words run together
        let cs: Vec<(usize, char)> = w.char_indices().collect();
        for &(i, _) in cs.iter().skip(1) {
            let (a, b) = (&w[..i], &w[i..]);
            let short_ok = |p: &str| p.chars().count() > 1 || p == "a" || p == "i";
            if short_ok(a) && short_ok(b) && d.suggestible(a) && d.suggestible(b) {
                put(format!("{} {}", a, b), 0.9, &mut cost);
            }
        }
        for c in d.near_words(&w, 60) {
            let k = dist(&w, &c.to_lowercase());
            if k <= 2.6 && k <= n as f32 * 0.5 && cost.get(&c).map_or(true, |&o| k < o) {
                cost.insert(c, k);
            }
        }
        cost.remove(&w);
        if case == Case::Lower {
            for (c, k) in cost.iter_mut() {
                if case_of(c) != Case::Lower && c.to_lowercase() != w {
                    *k += 0.3;
                }
            }
        }
        let mut v: Vec<(String, f32)> = cost.into_iter().collect();
        let sim = |c: &str| bigrams(&w, c);
        v.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap()
                .then_with(|| sim(&b.0).partial_cmp(&sim(&a.0)).unwrap())
                .then_with(|| (a.0.chars().count() as isize - n as isize).abs().cmp(&(b.0.chars().count() as isize - n as isize).abs()))
                .then_with(|| a.0.cmp(&b.0))
        });
        let mut out: Vec<String> = Vec::new();
        for (c, _) in v {
            let c = if case_of(&c) == Case::Lower { like(case, &c) } else { c };
            if !out.contains(&c) {
                out.push(c);
            }
            if out.len() == 12 {
                break;
            }
        }
        out
    }
}

/// British spellings and their American forms (`to_gb` runs them backwards).
const VARIANTS: &[(&str, &str)] = &[
    ("our", "or"),
    ("ise", "ize"),
    ("isi", "izi"),
    ("isa", "iza"),
    ("yse", "yze"),
    ("ysi", "yzi"),
    ("lled", "led"),
    ("lling", "ling"),
    ("ller", "ler"),
    ("tre", "ter"),
    ("ogue", "og"),
    ("ence", "ense"),
    ("grey", "gray"),
    ("mme", "m"),
    ("dgement", "dgment"),
    ("sceptic", "skeptic"),
    ("aem", "em"),
    ("paed", "ped"),
    ("oes", "es"),
    ("ae", "e"),
];

/// `w` respelt for the other side of the Atlantic: each rule on its own,
/// and all of them together.
fn variants(w: &str, to_gb: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut all = w.to_string();
    for (gb, us) in VARIANTS {
        let (a, b) = if to_gb { (us, gb) } else { (gb, us) };
        // short endings never start a word (ise, our); whole stems may (grey)
        let from = if a.len() >= 4 { 0 } else { 1 };
        if w.len() > a.len() && w.is_char_boundary(from) && w[from..].contains(a) {
            out.push(format!("{}{}", &w[..from], w[from..].replace(a, b)));
            if all.len() > a.len() && all.is_char_boundary(from) {
                all = format!("{}{}", &all[..from], all[from..].replace(a, b));
            }
        }
    }
    if out.len() > 1 {
        out.push(all);
    }
    out
}

/// Words every developer writes that general dictionaries lack (sorted).
const TECH: &[&str] = &[
    "abi", "api", "apis", "args", "async", "auth", "autocomplete", "autocompleted", "autocompletes", "autocompletion", "autoformat", "backend", "backends", "backoff", "backtick", "backticks", "bindgen", "bool", "boolean", "booleans", "bugfix", "bugfixes", "builtin", "builtins", "callback", "callbacks", "cfg", "changelog", "checkbox", "checkboxes", "cli", "codebase", "codebases", "config", "configs", "const", "cpp", "cpu", "crypto", "css", "csv", "dataset", "datasets", "dedent", "dedented", "dedents", "deduplicate", "deduplicated", "deps", "dev", "devs", "dict", "dicts", "distro", "distros", "dns", "docker", "dockerfile", "dropdown", "dropdowns", "email", "emails", "endpoint", "endpoints", "enum", "enums", "env", "eval", "favicon", "filename", "filenames", "filesystem", "filesystems", "fn", "frontend", "frontends", "fullscreen", "github", "gitignore", "glibc", "goto", "gpu", "hardcode", "hardcoded", "hashmap", "hostname", "html", "http", "https", "iframe", "iframes", "impl", "impls", "init", "inline", "inlined", "io", "ip", "ipv4", "ipv6", "javascript", "jpeg", "js", "json", "jwt", "keybinding", "keybindings", "keymap", "keymaps", "kubernetes", "lang", "lifecycle", "lifecycles", "linux", "localhost", "login", "logins", "logout", "lookup", "lookups", "lua", "makefile", "markdown", "metadata", "middleware", "minified", "minify", "mountpoint", "musl", "mutex", "namespace", "namespaces", "natively", "npm", "nullable", "oauth", "params", "parsers", "plaintext", "plugin", "plugins", "png", "posix", "postgres", "pre", "preload", "preloaded", "prepend", "prepended", "prepends", "presets", "prod", "programmatically", "pty", "ram", "readline", "readme", "realtime", "recolour", "recolours", "redis", "refactor", "refactored", "refactoring", "regex", "regexes", "repo", "repos", "rerender", "rerun", "reruns", "resize", "resized", "resizes", "resync", "resynced", "resyncing", "resyncs", "runtime", "runtimes", "rustc", "screenshot", "screenshots", "scrollable", "scrollback", "semver", "simd", "sql", "sqlite", "src", "ssd", "ssh", "ssl", "stderr", "stdin", "stdlib", "stdout", "strikethrough", "struct", "structs", "stylesheet", "stylesheets", "subcommand", "subcommands", "subdirectory", "subtask", "subtasks", "sudo", "svg", "symlink", "symlinks", "sys", "syscall", "syscalls", "systemd", "tcp", "theming", "timestamp", "timestamps", "tls", "tmux", "todo", "todos", "toml", "toolchain", "toolchains", "tooltip", "tooltips", "traceback", "tracebacks", "tuple", "tuples", "txt", "udp", "ui", "unicode", "unix", "untracked", "url", "urls", "username", "usernames", "userspace", "utf", "util", "utils", "ux", "vim", "vpn", "wasm", "webhook", "webhooks", "websocket", "websockets", "whitespace", "wifi", "workflow", "workflows", "workspace", "workspaces", "xml", "yaml", "zlib",
];

const COMMON: &[(&str, &str)] = &[
    ("alot", "a lot"),
    ("infact", "in fact"),
    ("aswell", "as well"),
    ("eachother", "each other"),
    ("incase", "in case"),
    ("inspite", "in spite"),
    ("atleast", "at least"),
    ("thankyou", "thank you"),
    ("noone", "no one"),
    ("alright", "all right"),
    ("irregardless", "regardless"),
    ("untill", "until"),
    ("wich", "which"),
    ("recieve", "receive"),
    ("seperate", "separate"),
    ("definately", "definitely"),
    ("occured", "occurred"),
    ("accomodate", "accommodate"),
    ("begining", "beginning"),
    ("beleive", "believe"),
    ("calender", "calendar"),
    ("enviroment", "environment"),
    ("goverment", "government"),
    ("independant", "independent"),
    ("neccessary", "necessary"),
    ("occassion", "occasion"),
    ("publically", "publicly"),
    ("tommorow", "tomorrow"),
    ("truely", "truly"),
    ("wierd", "weird"),
];

const KEYS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];

fn key_pos(c: char) -> Option<(i32, i32)> {
    KEYS.iter().enumerate().find_map(|(r, row)| row.find(c).map(|x| (r as i32, x as i32)))
}

fn near(a: char, b: char) -> bool {
    match (key_pos(a), key_pos(b)) {
        (Some((r1, x1)), Some((r2, x2))) => (r1 - r2).abs() <= 1 && (x1 - x2).abs() <= 1 && (r1, x1) != (r2, x2),
        _ => false,
    }
}

fn vowel(c: char) -> bool {
    "aeiouy".contains(c)
}

/// Every string one edit from `w`, with a cost that favours the mistakes
/// people make: swapped letters, doubled/undoubled letters, neighbouring
/// keys and vowel confusions. Edits to the first letter cost more.
fn edits(w: &str, alpha: &[char]) -> Vec<(String, f32)> {
    let cs: Vec<char> = w.chars().collect();
    let n = cs.len();
    let mut out = Vec::with_capacity(n * (2 * alpha.len() + 2) + alpha.len());
    let s = |v: &[char]| v.iter().collect::<String>();
    let first = |i: usize| if i == 0 { 0.4 } else { 0.0 };
    for i in 0..n {
        // delete
        let mut v = cs.clone();
        let c = v.remove(i);
        let doubled = (i > 0 && cs[i - 1] == c) || (i + 1 < n && cs[i + 1] == c);
        out.push((s(&v), if doubled { 0.5 } else { 1.0 } + first(i)));
        // transpose
        if i + 1 < n && cs[i] != cs[i + 1] {
            let mut v = cs.clone();
            v.swap(i, i + 1);
            out.push((s(&v), 0.6 + first(i)));
        }
        // replace
        for &a in alpha {
            if a != cs[i] {
                let mut v = cs.clone();
                v[i] = a;
                let k = if near(a, cs[i]) || (vowel(a) && vowel(cs[i])) { 0.8 } else { 1.0 };
                out.push((s(&v), k + first(i)));
            }
        }
    }
    // insert
    for i in 0..=n {
        for &a in alpha {
            let mut v = cs.clone();
            v.insert(i, a);
            let doubled = (i > 0 && cs[i - 1] == a) || (i < n && cs[i] == a);
            out.push((s(&v), if doubled { 0.5 } else { 1.0 } + first(i)));
        }
    }
    out
}

/// Edit distance (adjacent swaps count once) with the costs of `edits`.
fn dist(a: &str, b: &str) -> f32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0f32; m + 1]; n + 1];
    let indel = |s: &[char], i: usize| if (i > 0 && s[i - 1] == s[i]) || (i + 1 < s.len() && s[i + 1] == s[i]) { 0.5 } else { 1.0 };
    for i in 1..=n {
        d[i][0] = d[i - 1][0] + indel(&a, i - 1);
    }
    for j in 1..=m {
        d[0][j] = d[0][j - 1] + indel(&b, j - 1);
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = if a[i - 1] == b[j - 1] { 0.0 } else if near(a[i - 1], b[j - 1]) || (vowel(a[i - 1]) && vowel(b[j - 1])) { 0.8 } else { 1.0 };
            let mut v = (d[i - 1][j - 1] + sub).min(d[i - 1][j] + indel(&a, i - 1)).min(d[i][j - 1] + indel(&b, j - 1));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 0.6);
            }
            d[i][j] = v;
        }
    }
    d[n][m] + if a.first() != b.first() { 0.4 } else { 0.0 }
}

/// Shared letter pairs, 0..1.
fn bigrams(a: &str, b: &str) -> f32 {
    let pa: Vec<(char, char)> = a.chars().zip(a.chars().skip(1)).collect();
    let pb: Vec<(char, char)> = b.chars().zip(b.chars().skip(1)).collect();
    if pa.is_empty() || pb.is_empty() {
        return 0.0;
    }
    let common = pa.iter().filter(|p| pb.contains(p)).count();
    2.0 * common as f32 / (pa.len() + pb.len()) as f32
}

// ---------- what to skip ----------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Text,
    Markdown,
    /// A message draft: headers (only Subject is prose), a blank line, the
    /// body (quoted `>` lines are someone else's).
    Mail,
}

/// Per line: does it hold prose? Markdown code fences, front matter, math
/// blocks and multi-line HTML comments don't.
pub fn prose_lines(lines: &[String], kind: Kind) -> Vec<bool> {
    let mut out = vec![true; lines.len()];
    if kind == Kind::Mail {
        let mut head = true;
        for (i, l) in lines.iter().enumerate() {
            if head && l.trim().is_empty() {
                head = false;
                out[i] = false;
                continue;
            }
            out[i] = if head { l.starts_with("Subject:") } else { !l.trim_start().starts_with('>') };
        }
        return out;
    }
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("$ ") {
            out[i] = false; // a shell prompt
        }
    }
    if kind == Kind::Text {
        return out;
    }
    for (i, l) in lines.iter().enumerate() {
        if is_ref_definition(l) {
            out[i] = false;
        }
    }
    let mut prev_blank = true;
    let mut in_list = false;
    let mut indented_code = false;
    let mut fence: Option<(char, usize)> = None;
    let mut math = false;
    let mut comment = false;
    let mut front = lines.first().map_or(false, |l| l.trim_end() == "---");
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if front {
            out[i] = false;
            if i > 0 && (l.trim_end() == "---" || l.trim_end() == "...") {
                front = false;
            }
            continue;
        }
        if let Some((c, n)) = fence {
            out[i] = false;
            if t.starts_with(&c.to_string().repeat(n)) && t.trim_start_matches(c).trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if comment {
            out[i] = false;
            if l.contains("-->") {
                comment = false;
            }
            continue;
        }
        if math {
            out[i] = false;
            if t.trim_end() == "$$" {
                math = false;
            }
            continue;
        }
        let indent = l.len() - t.len();
        let blank = t.is_empty();
        // indented code: after a blank line, outside a list
        if !blank && (l.starts_with("    ") || l.starts_with('\t')) && (indented_code || (prev_blank && !in_list)) {
            indented_code = true;
            out[i] = false;
            prev_blank = false;
            continue;
        }
        if !blank {
            indented_code = false;
            if indent < 4 {
                let marker = t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || {
                    let d = t.bytes().take_while(|b| b.is_ascii_digit()).count();
                    d > 0 && (t[d..].starts_with(". ") || t[d..].starts_with(") "))
                };
                in_list = marker || (in_list && !prev_blank);
            }
        }
        prev_blank = blank;
        if indent < 4 {
            for c in ['`', '~'] {
                let n = t.chars().take_while(|&x| x == c).count();
                if n >= 3 {
                    fence = Some((c, n));
                }
            }
            if fence.is_some() {
                out[i] = false;
                continue;
            }
        }
        if t.trim_end() == "$$" {
            math = true;
            out[i] = false;
            continue;
        }
        if let Some(p) = l.rfind("<!--") {
            if !l[p..].contains("-->") {
                comment = true;
                out[i] = false;
            }
        }
    }
    out
}

/// A markdown line that can't continue the paragraph above it: headings,
/// list items, table rows, rules, HTML and the start of a quote.
fn block_start(l: &str, prev: &str) -> bool {
    let t = l.trim_start();
    let p = prev.trim_start();
    let rule = |t: &str| t.len() >= 3 && (t.chars().all(|c| c == '-' || c == ' ') || t.chars().all(|c| c == '=' || c == ' ') || t.chars().all(|c| c == '*' || c == ' ') || t.chars().all(|c| c == '_' || c == ' '));
    let list = |t: &str| {
        let d = t.bytes().take_while(|b| b.is_ascii_digit()).count();
        t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || (d > 0 && (t[d..].starts_with(". ") || t[d..].starts_with(") ")))
    };
    t.starts_with('#') || p.starts_with('#') || t.starts_with('|') || p.starts_with('|') || t.starts_with('<') || rule(t) || rule(p) || list(t) || t.starts_with('>') != p.starts_with('>')
}

/// Paragraphs: runs of prose lines (as line ranges) checked together, so a
/// problem can span a line break. Blank lines and non-prose lines end them,
/// and so do markdown block starts; very long runs are cut.
pub fn blocks(lines: &[String], kind: Kind) -> Vec<(usize, usize)> {
    let prose = prose_lines(lines, kind);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        let ok = prose[i] && !l.trim().is_empty();
        if let Some(a) = start {
            let cut = !ok || i - a >= 64 || (kind == Kind::Markdown && block_start(l, &lines[i - 1]));
            if cut {
                out.push((a, i));
                start = None;
            }
        }
        if ok && start.is_none() {
            start = Some(i);
        }
    }
    if let Some(a) = start {
        out.push((a, lines.len()));
    }
    out
}

fn is_ref_definition(line: &str) -> bool {
    let t = line.trim_start();
    line.len() - t.len() < 4 && t.starts_with('[') && t.find("]:").map_or(false, |i| !t[1..i].contains('['))
}

/// Byte ranges of a line that aren't prose: code spans, URLs, and in
/// markdown, link targets, HTML, entities, math and template tags.
fn skips(line: &str, md: bool) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    let find = |from: usize, pat: &str| line[from.min(n)..].find(pat).map(|k| from + k);
    while i < n {
        let c = b[i];
        let start = i;
        let end: Option<usize> = match c {
            b'`' => {
                let k = b[i..].iter().take_while(|&&x| x == b'`').count();
                let ticks = &line[i..i + k];
                let mut j = i + k;
                let mut close = None;
                while let Some(p) = find(j, ticks) {
                    let run = b[p..].iter().take_while(|&&x| x == b'`').count();
                    if run == k {
                        close = Some(p + k);
                        break;
                    }
                    j = p + run;
                }
                close.or(Some(i + k))
            }
            b'h' | b'f' | b'w' | b'm' if i == 0 || !b[i - 1].is_ascii_alphanumeric() => {
                let r = &line[i..];
                if ["http://", "https://", "ftp://", "file://", "www.", "mailto:"].iter().any(|p| r.starts_with(p)) {
                    Some(i + r.find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == ')' || c == ']').unwrap_or(r.len()))
                } else {
                    None
                }
            }
            b'{' if i + 1 < n && (b[i + 1] == b'{' || b[i + 1] == b'%') => {
                let close = if b[i + 1] == b'{' { "}}" } else { "%}" };
                find(i + 2, close).map(|p| p + 2)
            }
            b'<' if md && i + 1 < n && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'/' || b[i + 1] == b'!') => {
                if line[i..].starts_with("<!--") {
                    Some(find(i + 4, "-->").map_or(n, |p| p + 3))
                } else {
                    find(i + 1, ">").map(|p| p + 1)
                }
            }
            b']' if md && i + 1 < n && (b[i + 1] == b'(' || b[i + 1] == b'[') => {
                let (open, close) = if b[i + 1] == b'(' { (b'(', b')') } else { (b'[', b']') };
                let mut depth = 0;
                let mut j = i + 1;
                let mut end = n;
                while j < n {
                    if b[j] == open {
                        depth += 1;
                    } else if b[j] == close {
                        depth -= 1;
                        if depth == 0 {
                            end = j + 1;
                            break;
                        }
                    }
                    j += 1;
                }
                Some(end)
            }
            b'[' if md && i + 1 < n && (b[i + 1] == b'^' || b[i + 1] == b'[') => {
                let close = if b[i + 1] == b'[' { "]]" } else { "]" };
                find(i + 2, close).map(|p| p + close.len())
            }
            b'&' if md => {
                let k = b[i + 1..].iter().take_while(|x| x.is_ascii_alphanumeric() || **x == b'#').count();
                (k > 0 && b.get(i + 1 + k) == Some(&b';')).then_some(i + 2 + k)
            }
            b'$' if md && i + 1 < n && !b[i + 1].is_ascii_whitespace() && !b[i + 1].is_ascii_digit() => {
                let mut j = i + 1;
                let mut end = None;
                while let Some(p) = find(j, "$") {
                    if !b[p - 1].is_ascii_whitespace() {
                        end = Some(p + 1);
                        break;
                    }
                    j = p + 1;
                }
                end
            }
            b'\\' => Some((i + 2).min(n)),
            _ => None,
        };
        match end {
            Some(e) if e > start => {
                out.push((start, e));
                i = e;
            }
            _ => i += 1,
        }
    }
    out
}

// ---------- words ----------

#[derive(Debug)]
struct Tok {
    s: usize,
    e: usize,
    code: bool, // looks like code or an address, not a word
    lower: String,
}

fn joiner(c: char) -> bool {
    matches!(c, '\'' | '’' | '-' | '.' | '/' | '@' | ':' | '+')
}

fn tokens(line: &str, skip: &[(usize, usize)]) -> Vec<Tok> {
    let mut out = Vec::new();
    let cs: Vec<(usize, char)> = line.char_indices().collect();
    let mut sk = 0;
    let mut i = 0;
    let wordc = |c: char| c.is_alphanumeric() || c == '_';
    while i < cs.len() {
        let (p, c) = cs[i];
        while sk < skip.len() && skip[sk].1 <= p {
            sk += 1;
        }
        if sk < skip.len() && skip[sk].0 <= p {
            while i < cs.len() && cs[i].0 < skip[sk].1 {
                i += 1;
            }
            continue;
        }
        if !wordc(c) {
            i += 1;
            continue;
        }
        let mut j = i;
        loop {
            while j < cs.len() && wordc(cs[j].1) && !(sk < skip.len() && cs[j].0 >= skip[sk].0) {
                j += 1;
            }
            if j + 1 < cs.len() && joiner(cs[j].1) && wordc(cs[j + 1].1) && !(sk < skip.len() && cs[j + 1].0 >= skip[sk].0) {
                j += 1;
                continue;
            }
            break;
        }
        // emphasis underscores belong to markdown, not the word
        let (mut a, mut z) = (i, j);
        while a < z && cs[a].1 == '_' {
            a += 1;
        }
        while z > a && cs[z - 1].1 == '_' {
            z -= 1;
        }
        if a < z {
            let s = cs[a].0;
            let e = cs.get(z).map_or(line.len(), |x| x.0);
            let w = &line[s..e];
            let code = w.chars().any(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | '/' | '@' | ':' | '+'));
            let mut lower = String::with_capacity(w.len());
            for c in w.chars() {
                if c == '’' {
                    lower.push('\'');
                } else if c.is_ascii() {
                    lower.push(c.to_ascii_lowercase());
                } else {
                    lower.extend(c.to_lowercase());
                }
            }
            out.push(Tok { s, e, code, lower });
        }
        i = j;
    }
    out
}

/// The spell-checkable pieces of a word (hyphenated words are checked part
/// by part).
fn parts(line: &str, t: &Tok) -> Vec<(usize, usize)> {
    let w = &line[t.s..t.e];
    let mut out = Vec::new();
    let mut s = t.s;
    for p in w.split('-') {
        let (a, z) = (s, s + p.len());
        s = z + 1;
        let tp = p.trim_matches(|c| c == '\'' || c == '’');
        if !tp.is_empty() {
            let off = p.find(tp).unwrap_or(0);
            out.push((a + off, a + off + tp.len()));
        }
        let _ = z;
    }
    out
}

// ---------- grammar ----------

const ABBREV: &[&str] = &["e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "st", "cf", "approx", "no", "fig", "al", "inc", "ltd", "jr", "sr", "ca", "viz", "resp", "prof", "dept", "est", "eq", "ch", "sec", "vol", "pp", "p", "op", "cit", "ibid", "co", "corp", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "mt", "ft", "approx"];

/// Function words that follow a letter used as a name ("plan a or b"), so
/// an article in front of them is not an article.
const NOT_AFTER_ARTICLE: &[&str] = &["and", "or", "of", "in", "on", "is", "as", "at", "if", "it", "its", "are", "was", "an", "a", "the", "to", "for", "but", "by", "with", "from", "so", "than", "then", "vs", "nor", "into", "onto", "be", "has", "had", "were"];

/// "a" or "an" for the next word; None when the sound is not certain
/// (acronyms, symbols, words with both pronunciations).
fn article_for(word: &str) -> Option<&'static str> {
    let w = word.to_lowercase();
    let mut cs = w.chars();
    let f = cs.next()?;
    if w.chars().count() < 2 || !f.is_alphabetic() || matches!(case_of(word), Case::Upper | Case::Mixed) {
        return None;
    }
    if w.split('-').next().map_or(false, |p| p.chars().count() == 1) {
        return None; // x-ray, e-mail
    }
    let starts = |ps: &[&str]| ps.iter().any(|p| w.starts_with(p));
    if f == 'h' {
        if starts(&["hour", "honest", "honor", "honour", "heir"]) {
            return Some("an");
        }
        if starts(&["herb", "histor", "hotel", "hypot", "habitu", "hysteric", "heroic", "homage"]) {
            return None;
        }
        return Some("a");
    }
    if !"aeiou".contains(f) {
        return Some("a");
    }
    if w.starts_with("one") && !w.starts_with("oner") || w == "once" || starts(&["eu", "ewe", "use", "usu", "usa", "uti", "ura", "ure", "uri", "uro", "ubi", "uku", "ufo", "uto", "ute", "utu", "uta", "uvu", "ukr", "unanim"]) {
        return Some("a");
    }
    if let Some(r) = w.strip_prefix("uni") {
        return match r.chars().next() {
            Some(c) if "cfoqstvx".contains(c) => Some("a"),
            _ => Some("an"),
        };
    }
    Some("an")
}

struct Phrase {
    pat: &'static str,
    fix: &'static str,
    not_next: &'static [&'static str],
    at_end: bool,
}

const fn ph(pat: &'static str, fix: &'static str) -> Phrase {
    Phrase { pat, fix, not_next: &[], at_end: false }
}

const PHRASES: &[Phrase] = &[
    Phrase { pat: "could of", fix: "could have", not_next: &["course"], at_end: false },
    Phrase { pat: "should of", fix: "should have", not_next: &["course"], at_end: false },
    Phrase { pat: "would of", fix: "would have", not_next: &["course"], at_end: false },
    Phrase { pat: "must of", fix: "must have", not_next: &["course"], at_end: false },
    Phrase { pat: "might of", fix: "might have", not_next: &["course"], at_end: false },
    ph("more then", "more than"),
    ph("less then", "less than"),
    ph("better then", "better than"),
    ph("rather then", "rather than"),
    ph("greater then", "greater than"),
    ph("fewer then", "fewer than"),
    ph("worse then", "worse than"),
    ph("larger then", "larger than"),
    ph("smaller then", "smaller than"),
    ph("higher then", "higher than"),
    ph("lower then", "lower than"),
    ph("faster then", "faster than"),
    ph("slower then", "slower than"),
    ph("bigger then", "bigger than"),
    ph("easier then", "easier than"),
    ph("harder then", "harder than"),
    ph("shorter then", "shorter than"),
    ph("older then", "older than"),
    ph("younger then", "younger than"),
    ph("other then", "other than"),
    ph("its a", "it's a"),
    ph("its an", "it's an"),
    ph("its the", "it's the"),
    ph("its been", "it's been"),
    ph("its not", "it's not"),
    ph("its going", "it's going"),
    ph("it's own", "its own"),
    ph("it's self", "itself"),
    ph("your a", "you're a"),
    ph("your an", "you're an"),
    ph("your the", "you're the"),
    ph("your not", "you're not"),
    ph("your going", "you're going"),
    Phrase { pat: "your welcome", fix: "you're welcome", not_next: &[], at_end: true },
    ph("their is", "there is"),
    ph("their are", "there are"),
    ph("their was", "there was"),
    ph("their were", "there were"),
    ph("there own", "their own"),
    ph("he don't", "he doesn't"),
    ph("she don't", "she doesn't"),
    ph("it don't", "it doesn't"),
    ph("they was", "they were"),
    ph("we was", "we were"),
    ph("you was", "you were"),
    ph("was suppose to", "was supposed to"),
    ph("were suppose to", "were supposed to"),
    ph("is suppose to", "is supposed to"),
    ph("are suppose to", "are supposed to"),
    ph("am suppose to", "am supposed to"),
    ph("be suppose to", "be supposed to"),
    ph("per say", "per se"),
    ph("for all intensive purposes", "for all intents and purposes"),
    ph("case and point", "case in point"),
    ph("one in the same", "one and the same"),
    ph("deep seeded", "deep-seated"),
    ph("free reign", "free rein"),
    ph("sneak peak", "sneak peek"),
    ph("baited breath", "bated breath"),
    ph("tow the line", "toe the line"),
    ph("wreck havoc", "wreak havoc"),
    ph("make due", "make do"),
    ph("every since", "ever since"),
    ph("an other", "another"),
    ph("never the less", "nevertheless"),
    ph("allot of", "a lot of"),
    ph("all of the sudden", "all of a sudden"),
    ph("between you and i", "between you and me"),
    ph("peaked my interest", "piqued my interest"),
    ph("et all", "et al."),
];

/// The phrases that start with word `w`.
fn phrases_from(w: &str) -> impl Iterator<Item = &'static Phrase> {
    static BY_FIRST: std::sync::OnceLock<HashMap<&'static str, Vec<usize>>> = std::sync::OnceLock::new();
    let m = BY_FIRST.get_or_init(|| {
        let mut m: HashMap<&'static str, Vec<usize>> = HashMap::new();
        for (i, p) in PHRASES.iter().enumerate() {
            m.entry(p.pat.split(' ').next().unwrap()).or_default().push(i);
        }
        m
    });
    m.get(w).into_iter().flatten().map(|&i| &PHRASES[i])
}

const REPEAT_OK: &[&str] = &["blah", "bla", "that", "had", "is", "bye", "no", "ha", "yeah", "so", "very", "knock", "la", "do", "tom"];

fn grammar(line: &str, toks: &[Tok], skip: &[(usize, usize)], known: &dyn Fn(&str) -> bool, out: &mut Vec<Hit>) {
    let b = line.as_bytes();
    let gap = |i: usize| -> &str { &line[if i == 0 { 0 } else { toks[i - 1].e }..toks[i].s] };
    // only spaces between, and at most one line break (a paragraph's lines)
    let adjacent = |i: usize| i > 0 && !gap(i).is_empty() && gap(i).chars().all(|c| c == ' ' || c == '\t' || c == '\n') && gap(i).matches('\n').count() <= 1;
    let word = |t: &Tok| !t.code && t.lower.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-');
    let fix_case = |orig: &str, fix: &str| if orig.chars().next().map_or(false, |c| c.is_uppercase()) { title(fix) } else { fix.to_string() };
    for (i, t) in toks.iter().enumerate() {
        let text = &line[t.s..t.e];
        // doubled words
        if adjacent(i) && word(t) && t.lower == toks[i - 1].lower && !REPEAT_OK.contains(&t.lower.as_str()) {
            // across a line break: drop the second one (and its space), keep the break
            let (s, e) = if gap(i).contains('\n') { (t.s, t.e + line[t.e..].len() - line[t.e..].trim_start_matches(' ').len()) } else { (toks[i - 1].e, t.e) };
            out.push(Hit { s, e, grammar: true, msg: format!("repeated word \"{}\"", text), fixes: vec![String::new()] });
            continue;
        }
        // a / an
        if (t.lower == "a" || t.lower == "an") && i + 1 < toks.len() && adjacent(i + 1) && word(&toks[i + 1]) && !NOT_AFTER_ARTICLE.contains(&toks[i + 1].lower.as_str()) {
            let after_word = i > 0 && adjacent(i) && word(&toks[i - 1]);
            let next = &line[toks[i + 1].s..toks[i + 1].e];
            if let Some(want) = article_for(next).filter(|_| known(next)) {
                if want != t.lower && !(text == "A" && after_word) {
                    let f = fix_case(text, want);
                    out.push(Hit { s: t.s, e: t.e, grammar: true, msg: format!("use \"{}\" before \"{}\"", f, next), fixes: vec![f] });
                }
            }
        }
        // lower-case "i"
        if matches!(text, "i" | "i'm" | "i've" | "i'd" | "i'll" | "i’m" | "i’ve" | "i’d" | "i’ll") {
            let after = line[t.e..].chars().next();
            let before = line[..t.s].chars().next_back();
            let next = toks.get(i + 1).filter(|_| adjacent(i + 1)).map(|n| n.lower.as_str());
            let pronoun = text.len() > 1 || (next.is_some() && !matches!(next, Some("in" | "is" | "and" | "to" | "from" | "of" | "or" | "j" | "th" | "e")));
            if pronoun && !matches!(after, Some('.' | ')' | ']' | '=')) && before != Some('(') {
                let f = title(text);
                out.push(Hit { s: t.s, e: t.e, grammar: true, msg: format!("\"{}\" is always capitalised", f), fixes: vec![f] });
            }
        }
        // a sentence starts with a capital
        if i > 0 && word(t) && text.chars().next().map_or(false, |c| c.is_lowercase()) {
            let g = gap(i);
            let core = g.trim_end_matches([' ', '\t', '\n']);
            let ends = core.trim_end_matches(['"', '\'', '”', '’', ')', '*', '_']);
            let prev = &toks[i - 1];
            if core.len() < g.len()
                && (ends == "." || ends == "!" || ends == "?" || ends == "!?" || ends == "?!")
                && word(prev)
                && known(text)
                && prev.lower.chars().count() > 1
                && !ABBREV.contains(&prev.lower.as_str())
                && !line[prev.s..prev.e].chars().next().map_or(false, |c| c.is_uppercase() && prev.lower.chars().count() <= 3)
            {
                let f = title(text);
                out.push(Hit { s: t.s, e: t.e, grammar: true, msg: "a sentence starts with a capital letter".into(), fixes: vec![f] });
            }
        }
        // phrases
        for p in phrases_from(&t.lower) {
            let ws: Vec<&str> = p.pat.split(' ').collect();
            if i + ws.len() > toks.len() {
                continue;
            }
            if !(1..ws.len()).all(|k| adjacent(i + k) && toks[i + k].lower == ws[k]) {
                continue;
            }
            let last = i + ws.len() - 1;
            let next = toks.get(last + 1).filter(|_| adjacent(last + 1));
            if next.map_or(false, |n| p.not_next.contains(&n.lower.as_str())) {
                continue;
            }
            if p.at_end && next.is_some() {
                continue;
            }
            let f = fix_case(text, p.fix);
            out.push(Hit { s: t.s, e: toks[last].e, grammar: true, msg: format!("did you mean \"{}\"?", f), fixes: vec![f] });
        }
    }
    // punctuation spacing
    let in_skip = |p: usize| skip.iter().any(|&(s, e)| p >= s && p < e);
    for (k, c) in line.char_indices() {
        if !matches!(c, ',' | ';' | ':' | '.' | '!' | '?') || in_skip(k) {
            continue;
        }
        let after = line[k + 1..].chars().next();
        let before = line[..k].chars().next_back();
        // space before
        if before == Some(' ') && after.map_or(true, |a| a.is_whitespace()) && !(c == '.' && (after == Some('.') || line[..k].ends_with(" ."))) {
            let s = line[..k].trim_end_matches(' ').len();
            if s > 0 && line[..s].chars().next_back().map_or(false, |p| p.is_alphanumeric()) && !in_skip(s - 1) {
                out.push(Hit { s, e: k + 1, grammar: true, msg: format!("no space before '{}'", c), fixes: vec![c.to_string()] });
                continue;
            }
        }
        let bl = before.map_or(false, |p| p.is_alphabetic());
        // missing space after a comma
        if c == ',' && bl && after.map_or(false, |a| a.is_alphabetic()) && !in_skip(k + 1) {
            out.push(Hit { s: k, e: k + 1, grammar: true, msg: "missing space after ','".into(), fixes: vec![", ".into()] });
        }
        // doubled punctuation
        if (c == ',' || c == ';') && after == Some(c) {
            out.push(Hit { s: k, e: k + 2, grammar: true, msg: format!("doubled '{}'", c), fixes: vec![c.to_string()] });
        }
        if c == '.' && bl && after == Some('.') && line[k + 2..].chars().next().map_or(true, |a| a.is_whitespace()) && b.get(k + 2) != Some(&b'.') {
            out.push(Hit { s: k, e: k + 2, grammar: true, msg: "doubled '.'".into(), fixes: vec![".".into(), "...".into()] });
        }
    }
}

// ---------- in the editor ----------

use crate::editor::{Editor, Mode};
use crate::picker::{PickItem, Picker};

/// Buffers checked unless `:set spell`/`nospell` says otherwise.
fn prose(b: &crate::buffer::Buffer) -> bool {
    if b.lang.name == "markdown" {
        return true;
    }
    if b.lang.name != "text" {
        return false;
    }
    let Some(p) = &b.path else { return false };
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    matches!(ext_of(b).as_str(), "txt" | "text" | "eml") || matches!(name.as_str(), "COMMIT_EDITMSG" | "MERGE_MSG" | "TAG_EDITMSG" | "EDIT_DESCRIPTION")
}

fn ext_of(b: &crate::buffer::Buffer) -> String {
    b.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// A problem as the editor keeps it: the part on one line (`line`, `s..e`)
/// of a problem that runs from `from` to `to` (a grammar problem may span a
/// line break; then each line gets an entry and the later ones are `cont`).
#[derive(Clone)]
pub struct Issue {
    pub line: usize,
    pub s: usize,
    pub e: usize,
    pub cont: bool,
    pub from: (usize, usize),
    pub to: (usize, usize),
    hits: Rc<[Hit]>,
    k: usize,
}

impl Issue {
    pub fn hit(&self) -> &Hit {
        &self.hits[self.k]
    }
    pub fn grammar(&self) -> bool {
        self.hit().grammar
    }
    fn covers(&self, x: usize) -> bool {
        x >= self.s && x < self.e.max(self.s + 1)
    }
}

impl Editor {
    pub fn spell_active(&self) -> bool {
        let b = self.bb();
        b.spell_on.unwrap_or(self.opts.spell && prose(b))
    }

    /// Recheck the current buffer if it changed (cheap: paragraphs are
    /// cached).
    pub fn refresh_spell(&mut self) {
        if !self.spell_active() {
            let b = self.b();
            if !b.spell.is_empty() {
                b.spell.clear();
            }
            b.spell_key = (0, 0);
            return;
        }
        if let Some(e) = self.speller.ensure() {
            self.err(format!("spell: {}", e));
        }
        let gen = self.speller.gen;
        let (sp, b) = (&mut self.speller, &mut self.bufs[self.cur]);
        if b.spell_key == (b.version, gen) {
            return;
        }
        let md = b.lang.name == "markdown";
        let kind = if md { Kind::Markdown } else if ext_of(b) == "eml" { Kind::Mail } else { Kind::Text };
        let mut out = Vec::new();
        let mut starts = Vec::new();
        for (a, z) in blocks(&b.lines, kind) {
            let ls = &b.lines[a..z];
            let hits = sp.block(ls, md);
            if hits.is_empty() {
                continue;
            }
            starts.clear();
            let mut off = 0;
            for l in ls {
                starts.push(off);
                off += l.len() + 1;
            }
            let at = |p: usize| {
                let k = starts.partition_point(|&s| s <= p) - 1;
                (k, p - starts[k])
            };
            for (k, h) in hits.iter().enumerate() {
                let (l1, s1) = at(h.s);
                let (l2, last) = at(h.e.max(h.s + 1) - 1);
                let (from, to) = ((a + l1, s1), (a + l2, if l1 == l2 { h.e - starts[l1] } else { last + 1 }));
                let mut first = true;
                for l in l1..=l2 {
                    let s = if l == l1 { s1 } else { 0 };
                    let e = if l == l2 { to.1 } else { ls[l].len() };
                    if s >= e && l < l2 {
                        continue; // nothing of it on this line (it starts at the line break)
                    }
                    out.push(Issue { line: a + l, s, e, cont: !first, from, to, hits: hits.clone(), k });
                    first = false;
                }
            }
        }
        out.sort_by_key(|i| (i.line, i.s, i.cont));
        b.spell = out;
        b.spell_key = (b.version, gen);
    }

    /// Problems on line `y`, in order.
    pub fn spell_on_line(&self, y: usize) -> &[Issue] {
        let v = &self.bb().spell;
        let a = v.partition_point(|h| h.line < y);
        let z = v.partition_point(|h| h.line <= y);
        &v[a..z]
    }

    /// The problem under the cursor.
    pub fn spell_here(&self) -> Option<&Issue> {
        let (cy, cx) = self.cursor();
        self.spell_on_line(cy).iter().find(|h| h.covers(cx))
    }

    /// `]s` / `[s`: the next / previous problem, wrapping around.
    pub fn jump_spell(&mut self, fwd: bool, n: usize) {
        self.refresh_spell();
        if !self.spell_active() {
            return self.info("spell is off (:set spell)");
        }
        for _ in 0..n.max(1) {
            let c = self.cursor();
            let v: Vec<(usize, usize)> = self.bb().spell.iter().filter(|h| !h.cont).map(|h| (h.line, h.s)).collect();
            if v.is_empty() {
                return self.info("no spelling or grammar problems");
            }
            let target = if fwd { v.iter().find(|&&p| p > c).or(v.first()) } else { v.iter().rev().find(|&&p| p < c).or(v.last()) };
            let (y, x) = *target.unwrap();
            let wrapped = if fwd { (y, x) <= c } else { (y, x) >= c };
            self.last_jump = c;
            self.set_cursor((y, x.min(self.bb().lines[y].len().saturating_sub(1))));
            if wrapped {
                self.info(if fwd { "search hit BOTTOM, continuing at TOP" } else { "search hit TOP, continuing at BOTTOM" });
            }
        }
        let th = self.text_h();
        let b = self.b();
        if b.cy < b.top || b.cy >= b.top + th {
            b.top = b.cy.saturating_sub(th / 2);
        }
    }

    /// The word under the cursor as a byte range of its line.
    fn word_range(&self) -> Option<(usize, usize, usize)> {
        let (cy, cx) = self.cursor();
        let l = &self.bb().lines[cy];
        let wc = |c: char| c.is_alphanumeric() || c == '\'' || c == '’';
        let mut s = cx.min(l.len());
        while s < l.len() && !l.is_char_boundary(s) {
            s += 1;
        }
        if !l[s..].chars().next().map_or(false, wc) {
            return None;
        }
        while let Some(c) = l[..s].chars().next_back().filter(|&c| wc(c)) {
            s -= c.len_utf8();
        }
        let e = s + l[s..].find(|c: char| !wc(c)).unwrap_or(l.len() - s);
        let (s, e) = (s + (l[s..e].len() - l[s..e].trim_start_matches(['\'', '’']).len()), s + l[s..e].trim_end_matches(['\'', '’']).len());
        (s < e).then_some((cy, s, e))
    }

    /// `z=`: pick a correction for the problem or word under the cursor.
    pub fn spell_suggest(&mut self) {
        self.refresh_spell();
        if let Some(e) = self.speller.ensure() {
            return self.err(format!("spell: {}", e));
        }
        let here = self.spell_here().cloned();
        let (from, to, fixes, title) = match here {
            Some(i) if i.grammar() => (i.from, i.to, i.hit().fixes.clone(), i.hit().msg.clone()),
            other => {
                let (y, s, e) = match other {
                    Some(i) => (i.line, i.s, i.e),
                    None => match self.word_range() {
                        Some(r) => r,
                        None => return self.info("no word under the cursor"),
                    },
                };
                let w = self.bb().lines[y][s..e].to_string();
                let fixes = self.speller.suggest(&w);
                if fixes.is_empty() {
                    return self.info(format!("no suggestions for \"{}\"", w));
                }
                ((y, s), (y, e), fixes, format!("change \"{}\" to", w))
            }
        };
        let old = self.bb().text(from, to);
        let items = fixes
            .iter()
            .enumerate()
            .map(|(i, f)| PickItem { label: if f.is_empty() { format!("(delete \"{}\")", old.split_whitespace().collect::<Vec<_>>().join(" ")) } else { f.clone() }, path: None, buf: None, line: usize::MAX, col: i })
            .collect();
        self.spell_pick = Some((from, to, fixes));
        self.picker = Some(Picker::new(&title, items));
    }

    /// Replace the range `z=` was opened on with fix `i`.
    pub fn spell_apply(&mut self, i: usize) {
        let Some((from, to, fixes)) = self.spell_pick.take() else { return };
        let Some(f) = fixes.get(i) else { return };
        let b = self.bb();
        if to.0 >= b.lines.len() || to.1 > b.lines[to.0].len() || from.1 > b.lines[from.0].len() {
            return;
        }
        // a fix for words split across lines keeps the break after as many words
        let old = b.text(from, to);
        let f = match old.find('\n') {
            Some(nl) => {
                let n1 = old[..nl].split_whitespace().count();
                let ws: Vec<&str> = f.split(' ').collect();
                if n1 > 0 && n1 < ws.len() { format!("{}\n{}", ws[..n1].join(" "), ws[n1..].join(" ")) } else { f.clone() }
            }
            None => f.clone(),
        };
        let c = self.cursor();
        self.b().mark_start(c);
        self.b().delete(from, to);
        self.b().insert(from, &f);
        self.set_cursor(from);
        self.after_change();
    }

    /// `zg` (`save`: into ~/.config/mvi/words) / `zG` (this session):
    /// accept the word under the cursor.
    pub fn spell_good(&mut self, word: Option<&str>, save: bool) {
        let w = match word.map(str::trim).filter(|w| !w.is_empty()) {
            Some(w) => w.to_string(),
            None => {
                let here = self.spell_here().filter(|h| !h.grammar()).map(|h| (h.line, h.s, h.e));
                match here.or_else(|| self.word_range()) {
                    Some((y, s, e)) => self.bb().lines[y][s..e].to_string(),
                    None => return self.info("no word under the cursor"),
                }
            }
        };
        let _ = self.speller.ensure();
        match self.speller.add(&w, save) {
            Ok(()) => self.info(format!("\"{}\" added to {}", w, if save { "~/.config/mvi/words" } else { "this session's words" })),
            Err(e) => self.err(format!("spell: {}", e)),
        }
    }

    /// `:spell`: every problem in the buffer, in a picker.
    pub fn open_spell_picker(&mut self) {
        self.refresh_spell();
        let b = self.bb();
        let items: Vec<PickItem> = b
            .spell
            .iter()
            .filter(|i| !i.cont)
            .map(|i| {
                let what = if i.grammar() { i.hit().msg.clone() } else { format!("\"{}\"", &b.lines[i.line][i.s..i.e]) };
                let col = b.lines[i.line][..i.s].chars().count();
                PickItem { label: format!("{:>5}: {} {}", i.line + 1, if i.grammar() { "G" } else { "S" }, what), path: None, buf: Some(self.cur), line: i.line, col }
            })
            .collect();
        if items.is_empty() {
            return self.info(if self.spell_active() { "no spelling or grammar problems" } else { "spell is off (:set spell)" });
        }
        self.picker = Some(Picker::new("spelling and grammar", items));
    }

    /// One-line description of the problem under the cursor, for the
    /// message line.
    pub fn spell_message(&self) -> Option<(String, bool)> {
        if self.mode == Mode::Insert {
            return None;
        }
        let i = self.spell_here()?;
        Some(if i.grammar() {
            (format!("grammar: {}  (z= fix)", i.hit().msg), true)
        } else {
            (format!("spelling: \"{}\" is not in the dictionary  (z= suggestions, zg add)", &self.bb().lines[i.line][i.s..i.e]), false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFF: &str = "SET UTF-8\nTRY esiarntolcdugmfphbyvkw\nNOSUGGEST !\nPFX U Y 1\nPFX U 0 un .\nSFX S Y 4\nSFX S y ies [^aeiou]y\nSFX S 0 s [aeiou]y\nSFX S 0 es [sxzh]\nSFX S 0 s [^sxzhy]\nSFX D Y 4\nSFX D 0 d e\nSFX D y ied [^aeiou]y\nSFX D 0 ed [^ey]\nSFX D 0 ed [aeiou]y\nSFX M N 1\nSFX M 0 's .\nREP 2\nREP f ph\nREP ph f\n";
    const DIC: &str = "16\nthe\nsee\nand\ncarry/SDU\nlock/SDU\nbox/S\nParis/M\ncat/SM\nhello\nworld\nphone/S\nI\na\nan\nlot\nshit/!\n";

    fn sp() -> Speller {
        let mut s = Speller::default();
        s.dict = Some(Ok(Dict::new(AFF, DIC.into())));
        s
    }

    #[test]
    fn affixes_and_case() {
        let d = Dict::new(AFF, DIC.into());
        for w in ["carries", "carried", "uncarried", "unlocked", "boxes", "cat's", "Paris's", "Hello", "HELLO", "Paris", "PARIS"] {
            assert!(d.check(w), "{}", w);
        }
        for w in ["carrys", "boxs", "paris", "unbox", "helo", "lockes"] {
            assert!(!d.check(w), "{}", w);
        }
    }

    #[test]
    fn tech_words_are_sorted() {
        assert!(TECH.windows(2).all(|w| w[0] < w[1]), "{:?}", TECH.windows(2).find(|w| w[0] >= w[1]));
    }

    #[test]
    fn english_variants() {
        assert!(variants("colourised", false).contains(&"colorized".to_string()));
        assert!(variants("labelling", false).contains(&"labeling".to_string()));
        assert!(variants("color", true).contains(&"colour".to_string()));
    }

    #[test]
    fn suggestions() {
        let s = sp();
        assert_eq!(s.suggest("teh")[0], "the");
        assert_eq!(s.suggest("Teh")[0], "The");
        assert_eq!(s.suggest("paris")[0], "Paris");
        assert_eq!(s.suggest("fone")[0], "phone");
        assert_eq!(s.suggest("carryed")[0], "carried");
        assert!(s.suggest("helloworld").contains(&"hello world".to_string()));
        assert!(!s.suggest("shti").contains(&"shit".to_string()));
    }

    fn hits(s: &mut Speller, line: &str, md: bool) -> Vec<(String, bool)> {
        s.line(line, md).iter().map(|h| (line[h.s..h.e].to_string(), h.grammar)).collect()
    }

    #[test]
    fn spelling_skips_code_and_markup() {
        let mut s = sp();
        let h = hits(&mut s, "Hello wrld, see `fooo` and https://exmple.com/x and [the](http://zz) snake_case CamelCase API v2 a.b", true);
        assert_eq!(h, vec![("wrld".to_string(), false)]);
        let h = hits(&mut s, "the <span class=\"x\">world</span> &nbsp; $x^2$ [^nte] {{ tmpl }} _hello_", true);
        assert!(h.is_empty(), "{:?}", h);
        assert!(is_ref_definition("[id]: http://example.com"));
        let ind: Vec<String> = ["Text", "", "    code here", "    more code", "", "- item", "", "    item text", "", "$ make"].iter().map(|s| s.to_string()).collect();
        assert_eq!(prose_lines(&ind, Kind::Markdown), vec![true, true, false, false, true, true, true, true, true, false]);
        let lines: Vec<String> = ["---", "title: x", "---", "text", "```rust", "let x;", "```", "more"].iter().map(|s| s.to_string()).collect();
        assert_eq!(prose_lines(&lines, Kind::Markdown), vec![false, false, false, true, false, false, false, true]);
        let mail: Vec<String> = ["To: a@b.c", "Subject: Helo", "", "Body text", "> quoted txt"].iter().map(|s| s.to_string()).collect();
        assert_eq!(prose_lines(&mail, Kind::Mail), vec![false, true, false, true, false]);
    }

    fn grammar_of(line: &str) -> Vec<(String, Vec<String>)> {
        let mut s = Speller::default();
        s.dict = Some(Err(String::new()));
        s.line(line, true).iter().map(|h| (line[h.s..h.e].to_string(), h.fixes.clone())).collect()
    }

    #[test]
    fn paragraphs() {
        let md: Vec<String> = ["# Title", "first line of", "the paragraph", "", "- item one", "  continues here", "- item two", "| a | b |", "text"].iter().map(|s| s.to_string()).collect();
        assert_eq!(blocks(&md, Kind::Markdown), vec![(0, 1), (1, 3), (4, 6), (6, 7), (7, 8), (8, 9)]);
        let g = |ls: &[&str]| {
            let mut s = Speller::default();
            s.dict = Some(Err(String::new()));
            let ls: Vec<String> = ls.iter().map(|s| s.to_string()).collect();
            let text = ls.join("\n");
            s.block(&ls, true).iter().map(|h| text[h.s..h.e].to_string()).collect::<Vec<_>>()
        };
        assert_eq!(g(&["This is the", "the end. and", "it could", "of been an", "banana."]), vec!["the ", "and", "could\nof", "an"]);
        assert!(g(&["Plan A", "and plan B."]).is_empty());
    }

    #[test]
    fn grammar_rules() {
        let g = grammar_of("This is the the end.");
        assert_eq!(g, vec![(" the".into(), vec!["".into()])]);
        assert_eq!(grammar_of("It was a apple and an banana, a hour, an honest man, a union, an umbrella, a European."), vec![("a".into(), vec!["an".into()]), ("an".into(), vec!["a".into()]), ("a".into(), vec!["an".into()])]);
        assert!(grammar_of("Plan A and plan B. Option a or b. An MRI, a URL.").is_empty());
        assert_eq!(grammar_of("Yes i think so, and i'm sure. Loop for i in range."), vec![("i".into(), vec!["I".into()]), ("i'm".into(), vec!["I'm".into()])]);
        assert_eq!(grammar_of("It works. then it stops. See e.g. this, etc. and more... done"), vec![("then".into(), vec!["Then".into()])]);
        assert_eq!(grammar_of("You could of known it was more then that."), vec![("could of".into(), vec!["could have".into()]), ("more then".into(), vec!["more than".into()])]);
        assert!(grammar_of("You could of course do it.").is_empty());
        assert_eq!(grammar_of("Hello , world;; yes,no"), vec![(" ,".into(), vec![",".into()]), (";;".into(), vec![";".into()]), (",".into(), vec![", ".into()])]);
        assert!(grammar_of("Wait ... 1,000 items :) ok: fine `a , b`").is_empty());
        assert_eq!(grammar_of("Its a trap and your welcome").len(), 2);
    }
}

#[cfg(test)]
mod real {
    use super::*;

    /// cargo test -p mvi --release real_dictionary -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_dictionary() {
        let t = std::time::Instant::now();
        let mut s = Speller::default();
        s.lang = "en_US".into();
        assert!(s.ensure().is_none());
        println!("load {:?}", t.elapsed());
        let mut text = String::new();
        for f in std::env::var("SPELL_FILES").unwrap_or_default().split(':').filter(|f| !f.is_empty()) {
            text += &String::from_utf8_lossy(&std::fs::read(f).unwrap_or_default());
        }
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        let prose = prose_lines(&lines, Kind::Markdown);
        let t = std::time::Instant::now();
        let mut n = 0;
        let mut seen = std::collections::BTreeMap::new();
        let _ = &prose;
        for (a, z) in blocks(&lines, Kind::Markdown) {
            let l = &lines[a..z].join("\n");
            {
                for h in s.block(&lines[a..z], true).iter() {
                    n += 1;
                    let k = if h.grammar { format!("G {} -> {:?} ({})", &l[h.s..h.e], h.fixes, h.msg) } else { format!("S {}", &l[h.s..h.e]) };
                    *seen.entry(k).or_insert(0) += 1;
                }
            }
        }
        println!("{} lines, {} hits in {:?}", lines.len(), n, t.elapsed());
        // a big document: first pass, then what a keystroke costs (one line changed)
        let mut big: Vec<String> = Vec::new();
        for k in 0..60 {
            big.extend(lines.iter().map(|l| if l.trim_start().starts_with("```") || l.trim().is_empty() { l.clone() } else { format!("{} {}", l, k) }));
        }
        let mut s2 = Speller::default();
        s2.lang = "en_US".into();
        s2.ensure();
        let t = std::time::Instant::now();
        let p = prose_lines(&big, Kind::Markdown);
        let mut hits = 0;
        for (l, ok) in big.iter().zip(&p) {
            if *ok {
                hits += s2.line(l, true).len();
            }
        }
        println!("{} lines cold: {:?} ({} hits)", big.len(), t.elapsed(), hits);
        for (name, gram, dict) in [("no grammar", false, true), ("no dict", true, false), ("neither", false, false)] {
            let mut s3 = Speller::default();
            s3.lang = "en_US".into();
            s3.ensure();
            s3.grammar = gram;
            if !dict {
                s3.dict = Some(Err(String::new()));
            }
            let t = std::time::Instant::now();
            for l in &big {
                s3.line(l, true);
            }
            println!("  {}: {:?}", name, t.elapsed());
        }
        // the editor's path: paragraphs, then one line changed
        let mut s4 = Speller::default();
        s4.lang = "en_US".into();
        s4.ensure();
        let t = std::time::Instant::now();
        let mut hits = 0;
        for (a, z) in blocks(&big, Kind::Markdown) {
            hits += s4.block(&big[a..z], true).len();
        }
        println!("{} lines cold, by paragraph: {:?} ({} hits)", big.len(), t.elapsed(), hits);
        big[100].push_str(" teh");
        let t = std::time::Instant::now();
        let mut v = Vec::new();
        for (a, z) in blocks(&big, Kind::Markdown) {
            v.push(s4.block(&big[a..z], true));
        }
        println!("{} lines after one edit: {:?}", big.len(), t.elapsed());
        for (k, c) in seen {
            println!("{:3} {}", c, k);
        }
        let t = std::time::Instant::now();
        for w in ["teh", "recieve", "definately", "speling", "accomodation", "langauge", "wierd", "thier", "becuase", "existance", "occurence", "seperately", "untill", "grammer", "arguement"] {
            println!("{:12} {:?}", w, &s.suggest(w)[..s.suggest(w).len().min(5)]);
        }
        println!("suggest x15 {:?}", t.elapsed());
    }
}
