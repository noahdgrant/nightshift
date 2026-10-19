//! The time of day `ns auto` stops starting units.

use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone};

/// The next moment the clock reads `time` after `now`. At exactly `time` that is tomorrow's, so a
/// night started at 06:30 with `--until 06:30` does not stop at once.
pub fn deadline_find(now: DateTime<Local>, time: NaiveTime) -> DateTime<Local> {
    let today = now.date_naive();
    [today, today + Duration::days(1)]
        .into_iter()
        .filter_map(|day| Local.from_local_datetime(&day.and_time(time)).earliest())
        .find(|at| *at > now)
        .unwrap_or(now + Duration::days(1))
}

/// Reads `HH:MM` on the 24-hour clock.
pub fn time_parse(text: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(text, "%H:%M")
        .map_err(|_| format!("{text:?} is not a time of day like 06:30"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, day, hour, minute, 0)
            .earliest()
            .unwrap()
    }

    fn time(text: &str) -> NaiveTime {
        time_parse(text).unwrap()
    }

    #[test]
    fn a_time_later_today_is_today() {
        assert_eq!(deadline_find(at(10, 9, 0), time("17:30")), at(10, 17, 30));
    }

    #[test]
    fn a_time_already_passed_is_tomorrow() {
        assert_eq!(deadline_find(at(10, 22, 0), time("06:30")), at(11, 6, 30));
    }

    #[test]
    fn the_time_it_is_now_is_tomorrow() {
        assert_eq!(deadline_find(at(10, 6, 30), time("06:30")), at(11, 6, 30));
    }

    #[test]
    fn only_a_time_of_day_is_read() {
        assert!(time_parse("6:30").is_ok());
        for bad in ["", "25:00", "06:30:00", "tomorrow"] {
            assert!(time_parse(bad).is_err(), "{bad}");
        }
    }
}
