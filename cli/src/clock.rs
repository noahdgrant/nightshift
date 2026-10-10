//! Wall clock for `ns run` and `ns watch`. `NS_NOW` (unix seconds) pins it for tests; sleeps
//! then advance the pinned clock instead of blocking.

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Clock {
    pinned: Cell<Option<i64>>,
}

impl Clock {
    pub fn from_env() -> Self {
        Self {
            pinned: Cell::new(
                std::env::var("NS_NOW")
                    .ok()
                    .and_then(|v| v.trim().parse().ok()),
            ),
        }
    }

    pub fn now(&self) -> i64 {
        self.pinned.get().unwrap_or_else(real_now)
    }

    pub fn sleep_until(&self, t: i64) {
        match self.pinned.get() {
            Some(p) => self.pinned.set(Some(p.max(t))),
            None => {
                let d = t - real_now();
                if d > 0 {
                    std::thread::sleep(Duration::from_secs(d as u64));
                }
            }
        }
    }
}

fn real_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn local_tm(t: i64) -> libc::tm {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = t as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        tm
    }
}

/// The first local time `hh:mm` strictly after `now`.
#[cfg(unix)]
pub fn next_local(now: i64, hh: u32, mm: u32) -> i64 {
    let mut tm = local_tm(now);
    tm.tm_hour = hh as i32;
    tm.tm_min = mm as i32;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    // SAFETY: mktime normalises the struct we own.
    let mut t = unsafe { libc::mktime(&mut tm) } as i64;
    if t <= now {
        tm = local_tm(now);
        tm.tm_mday += 1;
        tm.tm_hour = hh as i32;
        tm.tm_min = mm as i32;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        // SAFETY: as above.
        t = unsafe { libc::mktime(&mut tm) } as i64;
    }
    t
}

#[cfg(not(unix))]
pub fn next_local(now: i64, hh: u32, mm: u32) -> i64 {
    let day = now - now.rem_euclid(86_400);
    let t = day + i64::from(hh) * 3600 + i64::from(mm) * 60;
    if t <= now {
        t + 86_400
    } else {
        t
    }
}

/// Parse `HH:MM`.
pub fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60 && s.len() <= 5).then_some((h, m))
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let s = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Unix seconds for an ISO-8601 time: `YYYY-MM-DD`, or with `THH:MM[:SS]` and an optional
/// `Z` or `±HH:MM` offset (none means UTC). `None` for anything else.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> {
        let t = s.get(a..b)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if s.get(4..5) != Some("-") || s.get(7..8) != Some("-") || !(1..=12).contains(&m) {
        return None;
    }
    if !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    let mut t = days_from_civil(y, m, d) * 86_400;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(t);
    }
    let time = rest.strip_prefix(['T', ' '])?;
    let (hh, mm) = (num(11, 13)?, num(14, 16)?);
    if time.get(2..3) != Some(":") || hh > 23 || mm > 59 {
        return None;
    }
    let mut zone = &s[16..];
    let mut ss = 0;
    if let Some(sec) = zone.strip_prefix(':') {
        ss = num(17, 19).filter(|v| *v < 60)?;
        zone = &sec[2..];
    }
    t += hh * 3600 + mm * 60 + ss;
    match zone {
        "" | "Z" => Some(t),
        _ => {
            let sign = match zone.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let (oh, om) = zone[1..].split_once(':')?;
            if oh.len() != 2 || om.len() != 2 {
                return None;
            }
            let off: i64 = oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60;
            Some(t - sign * off)
        }
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The local calendar date (`YYYY-MM-DD`) of unix time `t`, in the zone `TZ` names.
#[cfg(unix)]
pub fn local_date(t: i64) -> String {
    let tm = local_tm(t);
    format!(
        "{:04}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday
    )
}

#[cfg(not(unix))]
pub fn local_date(t: i64) -> String {
    iso(t)[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_and_hm() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_791_504_000), "2026-10-09T00:00:00Z");
        assert_eq!(parse_hm("06:30"), Some((6, 30)));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("6"), None);
    }

    #[test]
    fn pinned_clock_advances_on_sleep() {
        let c = Clock {
            pinned: Cell::new(Some(100)),
        };
        c.sleep_until(250);
        assert_eq!(c.now(), 250);
        c.sleep_until(10);
        assert_eq!(c.now(), 250);
    }

    #[test]
    fn parse_iso_reads_dates_times_and_offsets() {
        assert_eq!(parse_iso("2026-10-09T00:00:00Z"), Some(1_791_504_000));
        assert_eq!(parse_iso("2026-10-09"), Some(1_791_504_000));
        assert_eq!(parse_iso("2026-10-09T19:30:00Z"), Some(1_791_574_200));
        assert_eq!(parse_iso("2026-10-09T19:30Z"), Some(1_791_574_200));
        assert_eq!(parse_iso("2026-10-09 19:30:00"), Some(1_791_574_200));
        assert_eq!(parse_iso("2026-10-09T15:30:00-04:00"), Some(1_791_574_200));
        assert_eq!(parse_iso("2026-10-09T21:30:00+02:00"), Some(1_791_574_200));
        assert_eq!(parse_iso("2024-02-29"), Some(1_709_164_800));
        assert_eq!(parse_iso("2000-02-29"), Some(951_782_400));
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        for bad in [
            "",
            "2026-10",
            "2026/10/09",
            "2026-13-01",
            "2026-00-01",
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-10-00",
            "2026-10-09T24:00:00Z",
            "2026-10-09T10:60Z",
            "2026-10-09T10:00:60Z",
            "2026-10-09T10",
            "2026-10-09X10:00",
            "2026-10-09T10:00+0200",
            "2026-10-09T10:00*02:00",
            "2026-10/09",
            "2026-10-09T10-00Z",
            "2026-10-09T10:00+2:00",
            "2026-10-09T10:00+02:0",
            "2026-1a-09",
            "2026-10-09T10:00:00ZZ",
            "2026-10-09T1:00",
        ] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }

    #[test]
    fn iso_and_parse_iso_round_trip() {
        for t in [0, 951_782_400, 1_709_164_800, 1_791_574_200, 4_102_444_799] {
            assert_eq!(parse_iso(&iso(t)), Some(t));
        }
    }
}
