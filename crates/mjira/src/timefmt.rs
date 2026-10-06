// Jira timestamps (`2026-10-06T11:25:42.123+0000`) to ages and dates.

/// Days since 1970-01-01 for a civil date.
fn days(y: i64, m: i64, d: i64) -> i64 {
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// (year, month, day) of a day count since the epoch.
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Seconds since the epoch, honouring a trailing `Z`, `+0200` or `-07:00`.
pub fn epoch(iso: &str) -> Option<i64> {
    let n = |a: usize, b: usize| iso.get(a..b)?.parse::<i64>().ok();
    let t = days(n(0, 4)?, n(5, 7)?, n(8, 10)?) * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
    let mut rest = iso.get(19..).unwrap_or("");
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let sign = match rest.chars().next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return Some(t),
    };
    let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m) = (digits.get(0..2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0), digits.get(2..4).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0));
    Some(t - sign * (h * 3600 + m * 60))
}

pub fn age_at(iso: &str, now: i64) -> String {
    let Some(t) = epoch(iso) else { return String::new() };
    let s = (now - t).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        86400..=1209599 => format!("{}d", s / 86400),
        1209600..=5183999 => format!("{}w", s / 604800),
        _ => format!("{}mo", s / 2592000),
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

pub fn age(iso: &str) -> String {
    age_at(iso, now())
}

/// `3d ago`, or `just now`.
pub fn ago(iso: &str) -> String {
    match age(iso).as_str() {
        "now" => "just now".into(),
        a => format!("{} ago", a),
    }
}

/// `2026-10-06` for epoch milliseconds (an ADF date node).
pub fn date_ms(ms: i64) -> String {
    let (y, m, d) = civil(ms.div_euclid(86_400_000));
    format!("{:04}-{:02}-{:02}", y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epochs() {
        assert_eq!(epoch("1970-01-02T00:00:00.000+0000"), Some(86400));
        assert_eq!(epoch("2000-03-01T00:00:00Z"), Some(951868800));
        assert_eq!(epoch("2000-03-01T02:00:00.5+0200"), Some(951868800));
        assert_eq!(epoch("2000-02-29T17:00:00-07:00"), Some(951868800 - 86400 + 86400));
        assert_eq!(epoch("junk"), None);
    }

    #[test]
    fn ages() {
        let t = epoch("2026-10-06T11:25:42.000+0000").unwrap();
        assert_eq!(age_at("2026-10-06T11:25:42.000+0000", t + 90), "1m");
        assert_eq!(age_at("2026-10-06T11:25:42.000+0000", t + 3 * 86400), "3d");
        assert_eq!(age_at("garbage", t), "");
        assert_eq!(date_ms(951868800000), "2000-03-01");
    }
}
