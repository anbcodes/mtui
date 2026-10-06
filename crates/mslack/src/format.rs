// Slack mrkdwn → display text with styled spans, and the reverse for
// outgoing messages (@name → <@U…>, escaping).

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Span {
    Plain,
    Mention,
    MentionMe,
    Link,
    Code,
    Bold,
    Italic,
    Strike,
    Quote,
}

/// Display text plus one span tag per byte, and the URL behind each link.
#[derive(Default, Clone)]
pub struct Rich {
    pub text: String,
    pub spans: Vec<Span>,
    pub links: Vec<(usize, usize, String)>,
}

impl Rich {
    fn push(&mut self, s: &str, sp: Span) {
        self.text.push_str(s);
        self.spans.extend(std::iter::repeat(sp).take(s.len()));
    }
}

pub struct Names<'a> {
    pub users: &'a HashMap<String, String>,
    pub chans: &'a HashMap<String, String>,
    pub me: &'a str,
}

pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Resolve `<…>` references and entities, then mark up `code`, *bold*,
/// _italic_, ~strike~ and > quotes.
pub fn render(raw: &str, n: &Names) -> Rich {
    let mut r = Rich::default();
    let mut rest = raw;
    while let Some(i) = rest.find('<') {
        r.push(&emoji(&unescape(&rest[..i])), Span::Plain);
        let Some(j) = rest[i..].find('>') else {
            rest = &rest[i..];
            break;
        };
        let inner = &rest[i + 1..i + j];
        rest = &rest[i + j + 1..];
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) => (t, Some(unescape(l))),
            None => (inner, None),
        };
        if let Some(id) = target.strip_prefix('@') {
            let name = label.unwrap_or_else(|| n.users.get(id).cloned().unwrap_or_else(|| id.to_string()));
            r.push(&format!("@{}", name.trim_start_matches('@')), if id == n.me { Span::MentionMe } else { Span::Mention });
        } else if let Some(id) = target.strip_prefix('#') {
            let name = label.unwrap_or_else(|| n.chans.get(id).cloned().unwrap_or_else(|| id.to_string()));
            r.push(&format!("#{}", name), Span::Mention);
        } else if let Some(cmd) = target.strip_prefix('!') {
            let cmd = cmd.split('^').next().unwrap_or(cmd);
            let t = label.unwrap_or_else(|| format!("@{}", cmd.trim_start_matches("subteam")));
            r.push(&t, Span::MentionMe);
        } else {
            let url = unescape(target);
            let start = r.text.len();
            match label {
                Some(l) if l != url => r.push(&format!("{} ({})", l, url), Span::Link),
                _ => r.push(&url, Span::Link),
            }
            r.links.push((start, r.text.len(), url));
        }
    }
    r.push(&emoji(&unescape(rest)), Span::Plain);
    markup(&mut r);
    r
}

fn markup(r: &mut Rich) {
    let b = r.text.as_bytes();
    // ``` blocks and `inline code`
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' && r.spans[i] == Span::Plain {
            let fence = b[i..].starts_with(b"```");
            let w = if fence { 3 } else { 1 };
            let close = if fence { r.text[i + 3..].find("```") } else { r.text[i + 1..].find(['`', '\n']).filter(|&k| b[i + 1 + k] == b'`') };
            if let Some(k) = close {
                let end = i + w + k + w;
                r.spans[i..end].fill(Span::Code);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    // *bold* _italic_ ~strike~: markers at word boundaries, on one line
    for (m, sp) in [(b'*', Span::Bold), (b'_', Span::Italic), (b'~', Span::Strike)] {
        let mut i = 0;
        while i < b.len() {
            let left_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
            if b[i] == m && left_ok && r.spans[i] == Span::Plain && i + 1 < b.len() && b[i + 1] != b' ' {
                if let Some(k) = b[i + 1..].iter().position(|&c| c == m || c == b'\n') {
                    let e = i + 1 + k;
                    let right_ok = e + 1 >= b.len() || !b[e + 1].is_ascii_alphanumeric();
                    if b[e] == m && k > 0 && b[e - 1] != b' ' && right_ok && r.spans[i..=e].iter().all(|&s| s == Span::Plain) {
                        r.spans[i..=e].fill(sp);
                        i = e + 1;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }
    // > quoted lines
    let mut start = 0;
    for line in r.text.split('\n') {
        if line.starts_with('>') {
            for s in &mut r.spans[start..start + line.len()] {
                if *s == Span::Plain {
                    *s = Span::Quote;
                }
            }
        }
        start += line.len() + 1;
    }
}

/// Outgoing text: escape &<>, and turn @user / #channel / @here into Slack
/// references when they match a known name.
pub fn encode(text: &str, users: &HashMap<String, String>, chans: &HashMap<String, String>) -> String {
    let esc = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut out = String::with_capacity(esc.len());
    let mut it = esc.char_indices().peekable();
    let mut prev = ' ';
    while let Some((i, c)) = it.next() {
        if (c == '@' || c == '#') && !prev.is_alphanumeric() {
            let rest = &esc[i + 1..];
            let len = rest.find(|ch: char| !(ch.is_alphanumeric() || "._-'".contains(ch))).unwrap_or(rest.len());
            let word = rest[..len].trim_end_matches(['.', '\'']);
            let key = word.to_lowercase();
            let rep = match c {
                '@' if matches!(key.as_str(), "here" | "channel" | "everyone") => Some(format!("<!{}>", key)),
                '@' => users.get(&key).map(|id| format!("<@{}>", id)),
                _ => chans.get(&key).map(|id| format!("<#{}>", id)),
            };
            if let (Some(rep), false) = (rep, word.is_empty()) {
                out.push_str(&rep);
                while it.peek().is_some_and(|&(j, _)| j < i + 1 + word.len()) {
                    it.next();
                }
                prev = 'x';
                continue;
            }
        }
        out.push(c);
        prev = c;
    }
    out
}

pub const EMOJI: &[(&str, &str)] = &[
    ("+1", "👍"), ("thumbsup", "👍"), ("-1", "👎"), ("thumbsdown", "👎"), ("smile", "😄"), ("slightly_smiling_face", "🙂"),
    ("grinning", "😀"), ("laughing", "😆"), ("joy", "😂"), ("rolling_on_the_floor_laughing", "🤣"), ("wink", "😉"),
    ("blush", "😊"), ("heart_eyes", "😍"), ("thinking_face", "🤔"), ("neutral_face", "😐"), ("confused", "😕"),
    ("cry", "😢"), ("sob", "😭"), ("scream", "😱"), ("sweat_smile", "😅"), ("upside_down_face", "🙃"), ("sunglasses", "😎"),
    ("rage", "😡"), ("facepalm", "🤦"), ("shrug", "🤷"), ("pray", "🙏"), ("clap", "👏"), ("wave", "👋"), ("ok_hand", "👌"),
    ("muscle", "💪"), ("raised_hands", "🙌"), ("eyes", "👀"), ("heart", "❤️"), ("broken_heart", "💔"), ("fire", "🔥"),
    ("tada", "🎉"), ("rocket", "🚀"), ("star", "⭐"), ("sparkles", "✨"), ("zap", "⚡"), ("100", "💯"), ("white_check_mark", "✅"),
    ("heavy_check_mark", "✔️"), ("x", "❌"), ("warning", "⚠️"), ("question", "❓"), ("exclamation", "❗"), ("bulb", "💡"),
    ("memo", "📝"), ("bug", "🐛"), ("coffee", "☕"), ("beers", "🍻"), ("pizza", "🍕"), ("party_parrot", "🦜"),
    ("point_up", "☝️"), ("point_right", "👉"), ("point_left", "👈"), ("arrow_up", "⬆️"), ("arrow_down", "⬇️"),
    ("smiley", "😃"), ("stuck_out_tongue", "😛"), ("skull", "💀"), ("see_no_evil", "🙈"), ("hugging_face", "🤗"),
    ("melting_face", "🫠"), ("saluting_face", "🫡"), ("handshake", "🤝"), ("hourglass", "⌛"), ("calendar", "📅"),
    ("link", "🔗"), ("lock", "🔒"), ("bell", "🔔"), ("mag", "🔍"), ("chart_with_upwards_trend", "📈"), ("ship", "🚢"),
];

pub fn emoji_char(name: &str) -> Option<&'static str> {
    let base = name.split("::").next().unwrap_or(name);
    EMOJI.iter().find(|(n, _)| *n == base).map(|(_, e)| *e)
}

/// Replace known :shortcodes: with their emoji.
pub fn emoji(s: &str) -> String {
    if !s.contains(':') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(':') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || "_+-:".contains(c)) || c == ':');
        match end.filter(|&e| after.as_bytes().get(e) == Some(&b':') && e > 0).and_then(|e| emoji_char(&after[..e]).map(|ch| (e, ch))) {
            Some((e, ch)) => {
                out.push_str(ch);
                rest = &after[e + 1..];
                if let Some(t) = rest.strip_prefix(":skin-tone-").and_then(|t| t.split_once(':')) {
                    rest = t.1;
                }
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_refs() {
        let users: HashMap<String, String> = [("U1".to_string(), "alice".to_string())].into();
        let chans: HashMap<String, String> = [("C1".to_string(), "general".to_string())].into();
        let n = Names { users: &users, chans: &chans, me: "U1" };
        let r = render("hi <@U1> in <#C1> see <https://x.io|docs> &amp; :tada: *bold* `a*b*`", &n);
        assert_eq!(r.text, "hi @alice in #general see docs (https://x.io) & 🎉 *bold* `a*b*`");
        assert_eq!(r.spans[3], Span::MentionMe);
        let b = r.text.find("*bold*").unwrap();
        assert_eq!(r.spans[b], Span::Bold);
        let c = r.text.find("`a").unwrap();
        assert_eq!(r.spans[c + 2], Span::Code);
        let (a, b, u) = &r.links[0];
        assert_eq!((&r.text[*a..*b], u.as_str()), ("docs (https://x.io)", "https://x.io"));
    }

    #[test]
    fn encode_refs() {
        let users: HashMap<String, String> = [("alice".to_string(), "U1".to_string())].into();
        let chans: HashMap<String, String> = [("general".to_string(), "C1".to_string())].into();
        assert_eq!(encode("@alice: see #general, a<b @here email@x.com", &users, &chans), "<@U1>: see <#C1>, a&lt;b <!here> email@x.com");
    }

    #[test]
    fn emoji_codes() {
        assert_eq!(emoji("ok :+1: at 10:30 :nope:"), "ok 👍 at 10:30 :nope:");
    }
}
