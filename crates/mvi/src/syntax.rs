// Table-driven syntax highlighting with per-line state caching.

use std::path::Path;

pub const NORMAL: u8 = 0;
pub const KEYWORD: u8 = 1;
pub const TYPE: u8 = 2;
pub const FUNC: u8 = 3;
pub const STRING: u8 = 4;
pub const NUMBER: u8 = 5;
pub const COMMENT: u8 = 6;
pub const MACRO: u8 = 7;
pub const HEADING: u8 = 8;

pub struct Lang {
    pub name: &'static str,
    pub exts: &'static [&'static str],
    pub files: &'static [&'static str],
    pub line_comment: &'static str,
    pub block: (&'static str, &'static str),
    pub nested: bool,
    pub strings: &'static [(&'static str, &'static str, bool)],
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub consts: &'static [&'static str],
    pub caps_types: bool,
    pub indent: usize,
    pub check: &'static [&'static str],
    pub defs: &'static [&'static str],
    pub flags: u32,
}

pub const F_LIFETIME: u32 = 1; // rust 'a
pub const F_HASH_PRE: u32 = 2; // C preprocessor
pub const F_AT_DECOR: u32 = 4; // @decorator
pub const F_DOLLAR: u32 = 8; // $var
pub const F_MD: u32 = 16; // markdown headings
pub const F_TAGS: u32 = 32; // <tag>
pub const F_BANG_MACRO: u32 = 64; // ident!
pub const F_RUST_ATTR: u32 = 128; // #[...]
pub const F_KEYS: u32 = 256; // key: / key = (yaml/toml)
pub const F_INDENT_COLON: u32 = 512; // python-style block start

const fn lang(name: &'static str) -> Lang {
    Lang {
        name,
        exts: &[],
        files: &[],
        line_comment: "",
        block: ("", ""),
        nested: false,
        strings: &[],
        keywords: &[],
        types: &[],
        consts: &[],
        caps_types: false,
        indent: 4,
        check: &[],
        defs: &[],
        flags: 0,
    }
}

const C_STR: &[(&str, &str, bool)] = &[("\"", "\"", false), ("'", "'", false)];
const C_TYPES: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "ssize_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "FILE", "auto", "wchar_t",
];
const C_KW: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue", "return", "goto", "struct", "union", "enum", "typedef", "static", "extern", "const", "volatile", "inline", "register", "sizeof", "restrict",
    // C++
    "class", "public", "private", "protected", "virtual", "override", "template", "typename", "namespace", "using", "new", "delete", "try", "catch", "throw", "this", "operator", "friend", "constexpr", "noexcept", "explicit", "mutable", "decltype", "final", "co_await", "co_return", "co_yield", "static_cast", "dynamic_cast", "reinterpret_cast", "const_cast",
];

static LANGS: &[Lang] = &[
    Lang {
        exts: &["rs"],
        line_comment: "//",
        block: ("/*", "*/"),
        nested: true,
        strings: &[("b\"", "\"", true), ("r#\"", "\"#", true), ("r\"", "\"", true), ("\"", "\"", true), ("'", "'", false)],
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use", "where", "while", "union",
        ],
        types: &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"],
        consts: &["true", "false", "None", "Some", "Ok", "Err"],
        caps_types: true,
        check: &["Cargo.toml|cargo check --message-format=short", "rustc --edition 2021 --error-format=short --emit=metadata --out-dir /tmp/mvi-rustc {file}"],
        defs: &["fn", "struct", "enum", "trait", "type", "mod", "const", "static", "macro_rules!", "union"],
        flags: F_LIFETIME | F_BANG_MACRO | F_RUST_ATTR,
        ..lang("rust")
    },
    Lang {
        exts: &["c", "h"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: C_STR,
        keywords: C_KW,
        types: C_TYPES,
        consts: &["NULL", "true", "false", "nullptr"],
        check: &["cc -fsyntax-only -Wall -Wextra {file}"],
        defs: &["struct", "enum", "union", "typedef", "#define"],
        flags: F_HASH_PRE,
        ..lang("c")
    },
    Lang {
        exts: &["cc", "cpp", "cxx", "hpp", "hh", "hxx", "ino"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: C_STR,
        keywords: C_KW,
        types: C_TYPES,
        consts: &["NULL", "true", "false", "nullptr"],
        caps_types: true,
        check: &["c++ -fsyntax-only -Wall -std=c++20 {file}"],
        defs: &["struct", "class", "enum", "union", "typedef", "namespace", "#define"],
        flags: F_HASH_PRE,
        ..lang("cpp")
    },
    Lang {
        exts: &["py", "pyi", "pyw"],
        line_comment: "#",
        strings: &[("\"\"\"", "\"\"\"", true), ("'''", "'''", true), ("\"", "\"", false), ("'", "'", false)],
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match", "case", "self",
        ],
        types: &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object", "type"],
        consts: &["True", "False", "None"],
        caps_types: true,
        check: &["ruff check --output-format=concise --quiet {file}", "python3 -c \"import ast,sys; ast.parse(open(sys.argv[1]).read(), sys.argv[1])\" {file}"],
        defs: &["def", "class"],
        flags: F_AT_DECOR | F_INDENT_COLON,
        ..lang("python")
    },
    Lang {
        exts: &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: &[("\"", "\"", false), ("'", "'", false), ("`", "`", true)],
        keywords: &[
            "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "of", "return", "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while", "with", "yield", "interface", "type", "enum", "implements", "private", "public", "protected", "readonly", "as", "declare", "abstract", "namespace", "keyof",
        ],
        types: &["string", "number", "boolean", "any", "unknown", "never", "object", "bigint", "symbol"],
        consts: &["true", "false", "null", "undefined", "NaN", "Infinity"],
        caps_types: true,
        indent: 2,
        check: &["tsconfig.json|npx --no-install tsc --noEmit --pretty false", "node --check {file}"],
        defs: &["function", "class", "interface", "type", "enum", "const", "let", "var"],
        flags: F_AT_DECOR | F_TAGS,
        ..lang("javascript")
    },
    Lang {
        exts: &["go"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: &[("\"", "\"", false), ("'", "'", false), ("`", "`", true)],
        keywords: &[
            "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var",
        ],
        types: &["bool", "byte", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any"],
        consts: &["true", "false", "nil", "iota"],
        indent: 4,
        check: &["go.mod|go vet ./...", "gofmt -e -l {file}"],
        defs: &["func", "type", "var", "const"],
        ..lang("go")
    },
    Lang {
        exts: &["sh", "bash", "zsh", "ksh"],
        files: &[".bashrc", ".zshrc", ".profile", ".bash_profile", "PKGBUILD"],
        line_comment: "#",
        strings: &[("\"", "\"", true), ("'", "'", true)],
        keywords: &[
            "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "local", "export", "readonly", "declare", "set", "unset", "shift", "exit", "source", "echo", "cd", "test", "eval", "exec", "trap",
        ],
        consts: &["true", "false"],
        indent: 2,
        check: &["shellcheck -f gcc {file}", "bash -n {file}"],
        defs: &["function"],
        flags: F_DOLLAR,
        ..lang("sh")
    },
    Lang {
        exts: &["json", "jsonc", "geojson"],
        line_comment: "//",
        strings: &[("\"", "\"", false)],
        consts: &["true", "false", "null"],
        indent: 2,
        check: &["python3 -m json.tool {file} >/dev/null", "jq empty {file}"],
        ..lang("json")
    },
    Lang {
        exts: &["toml", "ini", "cfg", "conf"],
        files: &["Cargo.lock"],
        line_comment: "#",
        strings: &[("\"\"\"", "\"\"\"", true), ("\"", "\"", false), ("'", "'", false)],
        consts: &["true", "false"],
        flags: F_KEYS,
        ..lang("toml")
    },
    Lang {
        exts: &["yaml", "yml"],
        line_comment: "#",
        strings: &[("\"", "\"", false), ("'", "'", false)],
        consts: &["true", "false", "null", "yes", "no", "on", "off"],
        indent: 2,
        flags: F_KEYS,
        ..lang("yaml")
    },
    Lang {
        exts: &["md", "markdown"],
        strings: &[("```", "```", true), ("`", "`", false)],
        block: ("<!--", "-->"),
        indent: 2,
        flags: F_MD,
        ..lang("markdown")
    },
    Lang {
        exts: &["lua"],
        line_comment: "--",
        block: ("--[[", "]]"),
        strings: &[("[[", "]]", true), ("\"", "\"", false), ("'", "'", false)],
        keywords: &["and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local", "not", "or", "repeat", "return", "then", "until", "while"],
        consts: &["true", "false", "nil"],
        indent: 2,
        check: &["luac -p {file}"],
        defs: &["function", "local"],
        ..lang("lua")
    },
    Lang {
        exts: &["java", "kt", "kts", "scala", "cs", "swift", "dart"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: &[("\"\"\"", "\"\"\"", true), ("\"", "\"", false), ("'", "'", false)],
        keywords: &[
            "abstract", "assert", "break", "case", "catch", "class", "continue", "default", "do", "else", "enum", "extends", "final", "finally", "for", "if", "implements", "import", "instanceof", "interface", "native", "new", "package", "private", "protected", "public", "return", "static", "super", "switch", "synchronized", "this", "throw", "throws", "try", "var", "val", "fun", "while", "override", "when", "object", "data", "func", "let", "struct", "namespace", "using", "async", "await", "in", "is", "as",
        ],
        types: &["int", "long", "short", "byte", "float", "double", "boolean", "char", "void", "string", "String", "Int", "Long", "Boolean", "Double"],
        consts: &["true", "false", "null", "nil"],
        caps_types: true,
        check: &["javac -d /tmp/mvi-javac {file}"],
        defs: &["class", "interface", "enum", "fun", "func", "struct", "object"],
        flags: F_AT_DECOR,
        ..lang("java")
    },
    Lang {
        exts: &["rb", "rake", "gemspec"],
        files: &["Gemfile", "Rakefile"],
        line_comment: "#",
        block: ("=begin", "=end"),
        strings: &[("\"", "\"", true), ("'", "'", true)],
        keywords: &[
            "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end", "ensure", "for", "if", "in", "module", "next", "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then", "undef", "unless", "until", "when", "while", "yield", "require", "attr_accessor", "attr_reader",
        ],
        consts: &["true", "false", "nil"],
        caps_types: true,
        indent: 2,
        check: &["ruby -wc {file} >/dev/null"],
        defs: &["def", "class", "module"],
        ..lang("ruby")
    },
    Lang {
        exts: &["zig"],
        line_comment: "//",
        strings: &[("\"", "\"", false), ("'", "'", false)],
        keywords: &[
            "const", "var", "fn", "pub", "return", "if", "else", "while", "for", "switch", "break", "continue", "struct", "enum", "union", "error", "try", "catch", "defer", "errdefer", "comptime", "inline", "export", "extern", "test", "and", "or", "orelse", "unreachable", "usingnamespace", "async", "await",
        ],
        types: &["u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64", "bool", "void", "type", "anyerror", "anytype"],
        consts: &["true", "false", "null", "undefined"],
        caps_types: true,
        check: &["zig ast-check {file}"],
        defs: &["fn", "const", "var"],
        ..lang("zig")
    },
    Lang {
        exts: &["html", "htm", "xml", "svg", "vue", "svelte"],
        block: ("<!--", "-->"),
        strings: &[("\"", "\"", false), ("'", "'", false)],
        indent: 2,
        flags: F_TAGS,
        ..lang("html")
    },
    Lang {
        exts: &["css", "scss", "less"],
        line_comment: "//",
        block: ("/*", "*/"),
        strings: &[("\"", "\"", false), ("'", "'", false)],
        keywords: &["important", "media", "import", "keyframes", "from", "to"],
        indent: 2,
        flags: F_DOLLAR | F_AT_DECOR,
        ..lang("css")
    },
    Lang {
        exts: &["mk", "make"],
        files: &["Makefile", "makefile", "GNUmakefile", "Dockerfile", "Containerfile", "CMakeLists.txt"],
        line_comment: "#",
        strings: &[("\"", "\"", false), ("'", "'", false)],
        keywords: &["ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "include", "define", "endef", "export", "FROM", "RUN", "COPY", "ADD", "CMD", "ENTRYPOINT", "ENV", "ARG", "WORKDIR", "EXPOSE", "USER", "VOLUME", "LABEL"],
        flags: F_DOLLAR,
        ..lang("make")
    },
    Lang {
        exts: &["sql"],
        line_comment: "--",
        block: ("/*", "*/"),
        strings: &[("'", "'", false), ("\"", "\"", false)],
        keywords: &[
            "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop", "alter", "index", "join", "left", "right", "inner", "outer", "on", "and", "or", "not", "null", "group", "by", "order", "having", "limit", "as", "primary", "key", "foreign", "references", "union", "distinct", "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "DROP", "ALTER", "INDEX", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "AND", "OR", "NOT", "NULL", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "AS", "PRIMARY", "KEY", "UNION", "DISTINCT",
        ],
        indent: 2,
        ..lang("sql")
    },
];

static PLAIN: Lang = lang("text");

pub fn plain() -> &'static Lang {
    &PLAIN
}

pub fn by_name(name: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.name == name || l.exts.contains(&name))
}

pub fn detect(path: &Path, first_line: &str) -> &'static Lang {
    let fname = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    for l in LANGS {
        if l.files.contains(&fname.as_str()) || (!ext.is_empty() && l.exts.contains(&ext.as_str())) {
            return l;
        }
    }
    if let Some(sb) = first_line.strip_prefix("#!") {
        for (k, n) in [("python", "python"), ("bash", "sh"), ("/sh", "sh"), ("zsh", "sh"), ("node", "javascript"), ("ruby", "ruby"), ("lua", "lua")] {
            if sb.contains(k) {
                return by_name(n).unwrap_or(&PLAIN);
            }
        }
    }
    &PLAIN
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    #[default]
    Normal,
    Comment(u8),
    Str(u8),
}

#[derive(Default)]
pub struct Highlighter {
    states: Vec<State>,
}

impl Highlighter {
    pub fn invalidate(&mut self, line: usize) {
        self.states.truncate(line + 1);
    }

    pub fn line(&mut self, lang: &Lang, lines: &[String], l: usize, out: &mut Vec<u8>) {
        if self.states.is_empty() {
            self.states.push(State::Normal);
        }
        while self.states.len() <= l {
            let k = self.states.len() - 1;
            let st = highlight(lang, &lines[k], self.states[k], out);
            self.states.push(st);
        }
        highlight(lang, &lines[l], self.states[l], out);
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

pub fn highlight(lang: &Lang, line: &str, mut st: State, out: &mut Vec<u8>) -> State {
    let b = line.as_bytes();
    let n = b.len();
    out.clear();
    out.resize(n, NORMAL);
    let starts = |i: usize, s: &str| !s.is_empty() && b[i..].starts_with(s.as_bytes());
    let f = lang.flags;
    if f & F_MD != 0 && st == State::Normal {
        let t = line.trim_start();
        if t.starts_with('#') {
            out.fill(HEADING);
            return st;
        }
        if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("> ") {
            let o = n - t.len();
            out[o] = KEYWORD;
        }
    }
    if f & F_HASH_PRE != 0 && st == State::Normal && line.trim_start().starts_with('#') {
        out.fill(MACRO);
        // still highlight strings/comments inside
    }
    let mut i = 0;
    while i < n {
        match st {
            State::Comment(d) => {
                if starts(i, lang.block.1) {
                    let e = i + lang.block.1.len();
                    out[i..e].fill(COMMENT);
                    i = e;
                    st = if d > 1 { State::Comment(d - 1) } else { State::Normal };
                } else if lang.nested && starts(i, lang.block.0) {
                    let e = i + lang.block.0.len();
                    out[i..e].fill(COMMENT);
                    i = e;
                    st = State::Comment(d.saturating_add(1));
                } else {
                    out[i] = COMMENT;
                    i += 1;
                }
                continue;
            }
            State::Str(si) => {
                let close = lang.strings[si as usize].1;
                if b[i] == b'\\' && !close.ends_with('#') && lang.name != "markdown" {
                    let e = (i + 2).min(n);
                    out[i..e].fill(STRING);
                    i = e;
                } else if starts(i, close) {
                    let e = i + close.len();
                    out[i..e].fill(STRING);
                    i = e;
                    st = State::Normal;
                } else {
                    out[i] = STRING;
                    i += 1;
                }
                continue;
            }
            State::Normal => {}
        }
        let c = b[i];
        if starts(i, lang.line_comment) && (lang.line_comment != "#" || i == 0 || !is_ident(b[i - 1]) || lang.name != "sh") {
            out[i..].fill(COMMENT);
            break;
        }
        if starts(i, lang.block.0) {
            let e = i + lang.block.0.len();
            out[i..e].fill(COMMENT);
            i = e;
            st = State::Comment(1);
            continue;
        }
        let prev_ident = i > 0 && is_ident(b[i - 1]);
        if !prev_ident {
            if let Some(si) = lang.strings.iter().position(|s| starts(i, s.0)) {
                let open = lang.strings[si].0;
                if open == "'" && f & F_LIFETIME != 0 {
                    // char literal or lifetime?
                    let rest = &line[i + 1..];
                    let mut it = rest.char_indices();
                    let is_char = match it.next() {
                        Some((_, '\\')) => true,
                        Some((_, ch)) => rest[ch.len_utf8()..].starts_with('\''),
                        None => false,
                    };
                    if !is_char {
                        let mut j = i + 1;
                        while j < n && is_ident(b[j]) {
                            j += 1;
                        }
                        out[i..j].fill(MACRO);
                        i = j;
                        continue;
                    }
                }
                let e = i + open.len();
                out[i..e].fill(STRING);
                i = e;
                st = State::Str(si as u8);
                continue;
            }
        }
        if c.is_ascii_digit() && !prev_ident {
            let mut j = i + 1;
            while j < n && (is_ident(b[j]) || (b[j] == b'.' && j + 1 < n && b[j + 1].is_ascii_digit())) {
                j += 1;
            }
            out[i..j].fill(NUMBER);
            i = j;
            continue;
        }
        if is_ident(c) && !c.is_ascii_digit() {
            let mut j = i + 1;
            while j < n && is_ident(b[j]) {
                j += 1;
            }
            let w = &line[i..j];
            let mut k = j;
            while k < n && b[k] == b' ' {
                k += 1;
            }
            let next = if k < n { b[k] } else { 0 };
            let kind = if lang.keywords.contains(&w) {
                KEYWORD
            } else if lang.types.contains(&w) {
                TYPE
            } else if lang.consts.contains(&w) {
                NUMBER
            } else if f & F_BANG_MACRO != 0 && j < n && b[j] == b'!' && (j + 1 >= n || b[j + 1] != b'=') {
                j += 1;
                MACRO
            } else if f & F_KEYS != 0 && (next == b':' || next == b'=') && line[..i].trim().is_empty() {
                FUNC
            } else if next == b'(' {
                FUNC
            } else if lang.caps_types && c.is_ascii_uppercase() {
                if w.len() > 1 && w.bytes().all(|x| !x.is_ascii_lowercase()) {
                    NUMBER
                } else {
                    TYPE
                }
            } else {
                NORMAL
            };
            if kind != NORMAL {
                out[i..j].fill(kind);
            }
            i = j;
            continue;
        }
        if f & F_RUST_ATTR != 0 && c == b'#' && (starts(i, "#[") || starts(i, "#![")) {
            let j = line[i..].find(']').map_or(n, |k| i + k + 1);
            out[i..j].fill(MACRO);
            i = j;
            continue;
        }
        if (f & F_AT_DECOR != 0 && c == b'@') || (f & F_DOLLAR != 0 && c == b'$') {
            let mut j = i + 1;
            if j < n && (b[j] == b'{' || b[j] == b'(') {
                let close = if b[j] == b'{' { '}' } else { ')' };
                j = line[j..].find(close).map_or(n, |k| j + k + 1);
            } else {
                while j < n && (is_ident(b[j]) || b[j] == b'.') {
                    j += 1;
                }
            }
            out[i..j].fill(MACRO);
            i = j;
            continue;
        }
        if f & F_TAGS != 0 && c == b'<' && i + 1 < n && (b[i + 1] == b'/' || b[i + 1].is_ascii_alphabetic()) && (lang.name == "html" || i == 0 || b[i - 1] == b' ' || b[i - 1] == b'(' || b[i - 1] == b'>') {
            let mut j = i + 1;
            if b[j] == b'/' {
                j += 1;
            }
            while j < n && (is_ident(b[j]) || b[j] == b'-' || b[j] == b':' || b[j] == b'.') {
                j += 1;
            }
            out[i..j].fill(KEYWORD);
            i = j;
            continue;
        }
        i += 1;
    }
    if let State::Str(si) = st {
        if !lang.strings[si as usize].2 {
            st = State::Normal;
        }
    }
    st
}
