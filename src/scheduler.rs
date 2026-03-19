use crate::config::Schedule;
use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: u64 = 86400;
const SECS_PER_HOUR: u64 = 3600;

/// Target hour (UTC) for scheduled runs.
const TARGET_HOUR: u64 = 9;

/// Current UNIX timestamp in seconds.
pub fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Compute the absolute UNIX timestamp of the next scheduled run.
pub fn next_run_timestamp(schedule: Schedule) -> u64 {
    let now = now_timestamp();
    now + seconds_until_next_run(schedule)
}

/// Compute seconds until the next scheduled run.
pub fn seconds_until_next_run(schedule: Schedule) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    match schedule {
        Schedule::Daily => seconds_until_next_daily(now),
        Schedule::Weekly => seconds_until_next_weekly(now),
        Schedule::Monthly => seconds_until_next_monthly(now),
    }
}

/// Seconds until the next occurrence of TARGET_HOUR:00 UTC.
fn seconds_until_next_daily(now: u64) -> u64 {
    let today_target = (now / SECS_PER_DAY) * SECS_PER_DAY + TARGET_HOUR * SECS_PER_HOUR;
    if now < today_target {
        today_target - now
    } else {
        today_target + SECS_PER_DAY - now
    }
}

/// Seconds until next Monday at TARGET_HOUR:00 UTC.
/// UNIX epoch (Jan 1, 1970) was a Thursday, so (days_since_epoch + 3) % 7 gives 0=Monday.
fn seconds_until_next_weekly(now: u64) -> u64 {
    let days = now / SECS_PER_DAY;
    let dow = (days + 3) % 7; // 0=Mon, 6=Sun
    let days_until_monday = if dow == 0 { 0 } else { 7 - dow };
    let target = (days + days_until_monday) * SECS_PER_DAY + TARGET_HOUR * SECS_PER_HOUR;
    if now < target {
        target - now
    } else {
        target + 7 * SECS_PER_DAY - now
    }
}

/// Seconds until the 1st of the next month at TARGET_HOUR:00 UTC.
/// Uses a simple approach: find the current day-of-month and compute remaining days.
fn seconds_until_next_monthly(now: u64) -> u64 {
    let days_since_epoch = now / SECS_PER_DAY;

    // Compute year/month/day from days since epoch
    let (year, month, _day) = days_to_ymd(days_since_epoch);

    // Target: 1st of next month
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };

    let target_days = ymd_to_days(next_year, next_month, 1);
    let target = target_days * SECS_PER_DAY + TARGET_HOUR * SECS_PER_HOUR;

    if now < target {
        target - now
    } else {
        // Already past target — aim for month after next
        let (y2, m2) = if next_month == 12 {
            (next_year + 1, 1)
        } else {
            (next_year, next_month + 1)
        };
        let target_days2 = ymd_to_days(y2, m2, 1);
        target_days2 * SECS_PER_DAY + TARGET_HOUR * SECS_PER_HOUR - now
    }
}

fn is_leap_year(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Convert days since UNIX epoch to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }

    let mut m = 1;
    loop {
        let dim = days_in_month(y, m);
        if days < dim {
            break;
        }
        days -= dim;
        m += 1;
    }

    (y, m, days + 1) // day is 1-based
}

/// Convert (year, month, day) to days since UNIX epoch.
fn ymd_to_days(y: u64, m: u64, d: u64) -> u64 {
    let mut days = 0u64;
    for year in 1970..y {
        days += if is_leap_year(year) { 366 } else { 365 };
    }
    for month in 1..m {
        days += days_in_month(y, month);
    }
    days + d - 1 // day is 1-based
}

/// Format a UNIX timestamp (millis) as a human-readable UTC time string.
pub fn format_timestamp(millis: i64) -> String {
    if millis <= 0 {
        return "unknown time".to_string();
    }
    let secs = millis as u64 / 1000;
    let (y, m, d) = days_to_ymd(secs / SECS_PER_DAY);
    let hour = (secs % SECS_PER_DAY) / SECS_PER_HOUR;
    let minute = (secs % SECS_PER_HOUR) / 60;
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, m, d, hour, minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn test_ymd_roundtrip() {
        let days = ymd_to_days(2024, 3, 15);
        assert_eq!(days_to_ymd(days), (2024, 3, 15));
    }

    #[test]
    fn test_format_timestamp() {
        // 2021-01-31 09:01:58 UTC = 1612080118000 ms
        let ts = 1612080118000;
        let formatted = format_timestamp(ts);
        assert!(formatted.starts_with("2021-01-31"));
    }

    #[test]
    fn test_leap_year() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn test_days_to_ymd_known_dates() {
        // 2024-01-01 is day 19723 since epoch
        assert_eq!(days_to_ymd(ymd_to_days(2024, 1, 1)), (2024, 1, 1));
        assert_eq!(days_to_ymd(ymd_to_days(2000, 2, 29)), (2000, 2, 29)); // leap day
        assert_eq!(days_to_ymd(ymd_to_days(1970, 12, 31)), (1970, 12, 31));
        assert_eq!(days_to_ymd(ymd_to_days(2025, 6, 15)), (2025, 6, 15));
    }

    #[test]
    fn test_ymd_to_days_epoch() {
        assert_eq!(ymd_to_days(1970, 1, 1), 0);
        assert_eq!(ymd_to_days(1970, 1, 2), 1);
    }

    #[test]
    fn test_days_in_month_values() {
        assert_eq!(days_in_month(2024, 1), 31);
        assert_eq!(days_in_month(2024, 2), 29); // leap
        assert_eq!(days_in_month(2023, 2), 28); // non-leap
        assert_eq!(days_in_month(2024, 4), 30);
        assert_eq!(days_in_month(2024, 12), 31);
    }

    #[test]
    fn test_format_timestamp_epoch() {
        assert_eq!(format_timestamp(0), "unknown time");
        assert_eq!(format_timestamp(-100), "unknown time");
    }

    #[test]
    fn test_format_timestamp_known() {
        // 2024-03-15 14:30 UTC = 1710513000 seconds = 1710513000000 ms
        let ts = 1710513000000_i64;
        let formatted = format_timestamp(ts);
        assert_eq!(formatted, "2024-03-15 14:30 UTC");
    }

    #[test]
    fn test_seconds_until_next_daily_returns_positive() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let wait = seconds_until_next_daily(now);
        assert!(wait > 0);
        assert!(wait <= SECS_PER_DAY);
    }

    #[test]
    fn test_seconds_until_next_weekly_returns_positive() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let wait = seconds_until_next_weekly(now);
        assert!(wait > 0);
        assert!(wait <= 7 * SECS_PER_DAY);
    }

    #[test]
    fn test_seconds_until_next_monthly_returns_positive() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let wait = seconds_until_next_monthly(now);
        assert!(wait > 0);
        assert!(wait <= 62 * SECS_PER_DAY); // max ~2 months
    }

    #[test]
    fn test_daily_before_target() {
        // Monday 2024-03-18 at 08:00 UTC -> should wait 1 hour until 09:00
        let now = ymd_to_days(2024, 3, 18) * SECS_PER_DAY + 8 * SECS_PER_HOUR;
        let wait = seconds_until_next_daily(now);
        assert_eq!(wait, SECS_PER_HOUR);
    }

    #[test]
    fn test_daily_after_target() {
        // Monday 2024-03-18 at 10:00 UTC -> should wait 23 hours until next 09:00
        let now = ymd_to_days(2024, 3, 18) * SECS_PER_DAY + 10 * SECS_PER_HOUR;
        let wait = seconds_until_next_daily(now);
        assert_eq!(wait, 23 * SECS_PER_HOUR);
    }

    #[test]
    fn test_weekly_on_monday_before_target() {
        // Monday 2024-03-18 at 08:00 UTC -> should wait 1 hour
        let monday = ymd_to_days(2024, 3, 18);
        let dow = (monday + 3) % 7;
        assert_eq!(dow, 0); // confirm it's Monday
        let now = monday * SECS_PER_DAY + 8 * SECS_PER_HOUR;
        let wait = seconds_until_next_weekly(now);
        assert_eq!(wait, SECS_PER_HOUR);
    }

    #[test]
    fn test_weekly_on_tuesday() {
        // Tuesday 2024-03-19 at 10:00 UTC -> should wait until next Monday 09:00
        let tuesday = ymd_to_days(2024, 3, 19);
        let dow = (tuesday + 3) % 7;
        assert_eq!(dow, 1); // confirm it's Tuesday
        let now = tuesday * SECS_PER_DAY + 10 * SECS_PER_HOUR;
        let wait = seconds_until_next_weekly(now);
        // 6 days - 1 hour = 5 days 23 hours
        assert_eq!(wait, 6 * SECS_PER_DAY - SECS_PER_HOUR);
    }

    #[test]
    fn test_ymd_roundtrip_many_dates() {
        for y in [1970, 1999, 2000, 2001, 2024, 2100] {
            for m in [1, 2, 6, 12] {
                let d = 1;
                let days = ymd_to_days(y, m, d);
                assert_eq!(days_to_ymd(days), (y, m, d), "Failed for {}-{}-{}", y, m, d);
            }
        }
    }
}
