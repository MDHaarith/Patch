//! Floating local date/time helpers.
//!
//! Dates are stored as "floating" local times (`2026-10-13T10:00`): a task at
//! 10:00 stays at 10:00 wherever the device is, which is what people expect
//! from a personal planner.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

pub const DT_FMT: &str = "%Y-%m-%dT%H:%M";
pub const DATE_FMT: &str = "%Y-%m-%d";

pub fn fmt_dt(dt: NaiveDateTime) -> String {
    dt.format(DT_FMT).to_string()
}

pub fn fmt_date(d: NaiveDate) -> String {
    d.format(DATE_FMT).to_string()
}

pub fn parse_dt(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, DT_FMT)
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
        .or_else(|| parse_date(s).map(|d| d.and_time(NaiveTime::MIN)))
}

pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, DATE_FMT).ok()
}

pub fn now_local() -> NaiveDateTime {
    let now = chrono::Local::now().naive_local();
    now.with_second(0).and_then(|d| d.with_nanosecond(0)).unwrap_or(now)
}

pub fn today() -> NaiveDate {
    now_local().date()
}

pub fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h.min(23), m.min(59), 0).unwrap_or(NaiveTime::MIN)
}

/// Minutes since midnight.
pub fn minutes_of(dt: NaiveDateTime) -> i64 {
    dt.hour() as i64 * 60 + dt.minute() as i64
}

/// First day of the week containing `d`.
pub fn week_start(d: NaiveDate, monday_first: bool) -> NaiveDate {
    let offset = if monday_first {
        d.weekday().num_days_from_monday()
    } else {
        d.weekday().num_days_from_sunday()
    };
    d - Duration::days(offset as i64)
}

pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// Adds calendar months, clamping the day to the end of shorter months.
pub fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    if months >= 0 {
        d.checked_add_months(Months::new(months as u32)).unwrap_or(d)
    } else {
        d.checked_sub_months(Months::new(months.unsigned_abs())).unwrap_or(d)
    }
}

/// The 42 days (6 weeks) shown by a month grid containing `d`.
pub fn month_grid(d: NaiveDate, monday_first: bool) -> Vec<NaiveDate> {
    let first = week_start(month_start(d), monday_first);
    (0..42).map(|i| first + Duration::days(i)).collect()
}

pub fn days_in_month(d: NaiveDate) -> u32 {
    let start = month_start(d);
    (add_months(start, 1) - start).num_days() as u32
}

/// "10:00" or "10am" depending on the clock preference.
pub fn fmt_time(dt: NaiveDateTime, h24: bool) -> String {
    if h24 {
        dt.format("%H:%M").to_string()
    } else {
        let h = dt.hour() % 12;
        let h = if h == 0 { 12 } else { h };
        let suffix = if dt.hour() < 12 { "am" } else { "pm" };
        if dt.minute() == 0 {
            format!("{h}{suffix}")
        } else {
            format!("{h}:{:02}{suffix}", dt.minute())
        }
    }
}

pub fn fmt_hour_label(hour: u32, h24: bool) -> String {
    if h24 {
        format!("{hour:02}:00")
    } else {
        match hour {
            0 => "12 AM".into(),
            12 => "Noon".into(),
            h if h < 12 => format!("{h} AM"),
            h => format!("{} PM", h - 12),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_on_week_start() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let grid = month_grid(d, true);
        assert_eq!(grid[0], NaiveDate::from_ymd_opt(2026, 9, 28).unwrap());
        assert_eq!(grid[3], NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        let sunday = month_grid(d, false);
        assert_eq!(sunday[0], NaiveDate::from_ymd_opt(2026, 9, 27).unwrap());
    }

    #[test]
    fn months_clamp() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(add_months(d, 1), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(add_months(d, -2), NaiveDate::from_ymd_opt(2025, 11, 30).unwrap());
        assert_eq!(days_in_month(d), 31);
    }

    #[test]
    fn round_trip() {
        let dt = parse_dt("2026-10-13T10:00").unwrap();
        assert_eq!(fmt_dt(dt), "2026-10-13T10:00");
        assert_eq!(fmt_time(dt, false), "10am");
        assert_eq!(parse_dt("2026-10-13").unwrap().date(), dt.date());
    }
}
