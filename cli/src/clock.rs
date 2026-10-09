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
}
