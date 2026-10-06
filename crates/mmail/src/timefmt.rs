// Dates: parse JMAP UTC timestamps and show them in local time.

/// Seconds since the epoch for `2026-10-06T11:25:42Z`.
pub fn epoch(iso: &str) -> Option<i64> {
    let n = |a: usize, b: usize| iso.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + n(11, 13).unwrap_or(0) * 3600 + n(14, 16).unwrap_or(0) * 60 + n(17, 19).unwrap_or(0))
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// (year, month 1-12, day, hour, minute, weekday 0=Sunday) in local time.
pub fn local(t: i64) -> (i64, usize, i64, i64, i64, usize) {
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&tt, &mut tm);
        (tm.tm_year as i64 + 1900, tm.tm_mon as usize + 1, tm.tm_mday as i64, tm.tm_hour as i64, tm.tm_min as i64, tm.tm_wday as usize)
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// List-row style: the time today, "Oct 5" this year, else "2025-10-05".
pub fn short_at(iso: &str, now: i64) -> String {
    let Some(t) = epoch(iso) else { return String::new() };
    let (y, mo, d, h, mi, _) = local(t);
    let (ny, nmo, nd, ..) = local(now);
    if (y, mo, d) == (ny, nmo, nd) {
        format!("{:02}:{:02}", h, mi)
    } else if y == ny {
        format!("{} {}", MONTHS[mo - 1], d)
    } else {
        format!("{}-{:02}-{:02}", y, mo, d)
    }
}

/// "Mon, Oct 5 2026 15:04"
pub fn long(iso: &str) -> String {
    let Some(t) = epoch(iso) else { return iso.to_string() };
    let (y, mo, d, h, mi, wd) = local(t);
    format!("{}, {} {} {} {:02}:{:02}", DAYS[wd], MONTHS[mo - 1], d, y, h, mi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2026-10-06T11:25:42Z"), Some(1791285942));
        assert_eq!(epoch("garbage"), None);
    }
}
