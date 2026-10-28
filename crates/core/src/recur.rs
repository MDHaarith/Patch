//! Expansion of repeating tasks into concrete occurrences.

use crate::model::{Repeat, Task, Unit};
use crate::time::add_months;
use chrono::{Datelike, Duration, NaiveDateTime, Weekday};

#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub start: NaiveDateTime,
    /// Exclusive end.
    pub end: NaiveDateTime,
    /// 0 for the task's own dates, >0 for generated repeats.
    pub index: u32,
}

const MAX_OCCURRENCES: u32 = 800;

/// The `k`-th occurrence start for a repeat rule anchored at `base`.
fn nth(base: NaiveDateTime, repeat: Repeat, k: u32, anchor: Option<NaiveDateTime>) -> Option<NaiveDateTime> {
    let k64 = k as i64;
    let calendar = |months| {
        let anchor = anchor.unwrap_or(base);
        let offset = (base.year() - anchor.year()) * 12 + base.month() as i32 - anchor.month() as i32;
        add_months(anchor.date(), offset + months).and_time(base.time())
    };
    Some(match repeat {
        Repeat::None => {
            if k == 0 {
                base
            } else {
                return None;
            }
        }
        Repeat::Daily => base + Duration::days(k64),
        Repeat::Weekly => base + Duration::weeks(k64),
        Repeat::Monthly => calendar(k as i32),
        Repeat::Yearly => calendar(12 * k as i32),
        Repeat::Every(n, unit) => {
            let steps = n as i64 * k64;
            match unit {
                Unit::Day => base + Duration::days(steps),
                Unit::Week => base + Duration::weeks(steps),
                Unit::Month => calendar(steps as i32),
                Unit::Year => calendar(12 * steps as i32),
            }
        }
        Repeat::Weekdays => {
            // Walk weekday by weekday; callers jump close to the window first.
            let mut d = base;
            let mut left = k;
            while left > 0 {
                d += Duration::days(1);
                if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
                    left -= 1;
                }
            }
            d
        }
    })
}

/// Occurrences of `task` overlapping `[from, to)`.
pub fn occurrences(task: &Task, from: NaiveDateTime, to: NaiveDateTime) -> Vec<Occurrence> {
    let (Some(start), Some(_)) = (task.start, task.span_end()) else {
        return Vec::new();
    };
    let dur = task.duration();
    let mut out = Vec::new();

    // Skip ahead for fixed-length rules so long-running repeats stay cheap.
    let fixed_step = match task.repeat {
        Repeat::Daily => Some(1),
        Repeat::Weekly => Some(7),
        Repeat::Every(n, Unit::Day) => Some(n as i64),
        Repeat::Every(n, Unit::Week) => Some(7 * n as i64),
        _ => None,
    };
    let mut k: u32 = match fixed_step {
        Some(step) => {
            let gap = (from - dur - start).num_days();
            if gap > step {
                (gap / step - 1).max(0) as u32
            } else {
                0
            }
        }
        None => 0,
    };

    let mut base = start;
    if task.repeat == Repeat::Weekdays {
        let gap_days = (from - dur - start).num_days();
        if gap_days > 7 {
            let weeks = gap_days / 7 - 1;
            base = start + Duration::weeks(weeks);
            k = 0;
        }
    }

    let mut produced = 0;
    while produced < MAX_OCCURRENCES {
        let Some(s) = nth(base, task.repeat, k, task.repeat_anchor) else { break };
        if s >= to {
            break;
        }
        let e = s + dur;
        if e > from {
            out.push(Occurrence { start: s, end: e, index: k + (base != start) as u32 });
        }
        k += 1;
        produced += 1;
    }
    out
}

/// Start of the next occurrence after the task's current one.
pub fn next_start(task: &Task) -> Option<NaiveDateTime> {
    let start = task.start?;
    nth(start, task.repeat, 1, task.repeat_anchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_dt;

    fn task(start: &str, end: Option<&str>, repeat: Repeat) -> Task {
        let mut t = Task::blank("t", "l", "c");
        t.start = parse_dt(start);
        t.end = end.and_then(parse_dt);
        t.repeat = repeat;
        t
    }

    #[test]
    fn occurrences_keep_anchor_after_clamped_completion() {
        let mut task = task("2026-02-28T10:00", None, Repeat::Monthly);
        task.repeat_anchor = parse_dt("2026-01-31T10:00");
        let occ = occurrences(&task, parse_dt("2026-02-01").unwrap(), parse_dt("2026-05-01").unwrap());
        let starts: Vec<_> = occ.iter().map(|o| Some(o.start)).collect();
        assert_eq!(starts, vec![parse_dt("2026-02-28T10:00"), parse_dt("2026-03-31T10:00"), parse_dt("2026-04-30T10:00")]);
    }

    #[test]
    fn single_task_overlap() {
        let t = task("2026-10-13T10:00", Some("2026-10-13T11:00"), Repeat::None);
        let occ = occurrences(&t, parse_dt("2026-10-13").unwrap(), parse_dt("2026-10-14").unwrap());
        assert_eq!(occ.len(), 1);
        let none = occurrences(&t, parse_dt("2026-10-14").unwrap(), parse_dt("2026-10-15").unwrap());
        assert!(none.is_empty());
    }

    #[test]
    fn all_day_range_spans_days() {
        let mut t = task("2026-10-13", Some("2026-10-15"), Repeat::None);
        t.all_day = true;
        let occ = occurrences(&t, parse_dt("2026-10-15").unwrap(), parse_dt("2026-10-16").unwrap());
        assert_eq!(occ.len(), 1);
        assert_eq!(occ[0].end, parse_dt("2026-10-16").unwrap());
    }

    #[test]
    fn daily_and_weekdays() {
        let t = task("2026-01-01T09:00", None, Repeat::Daily);
        let occ = occurrences(&t, parse_dt("2026-10-01").unwrap(), parse_dt("2026-10-08").unwrap());
        assert_eq!(occ.len(), 7);
        assert_eq!(occ[0].start, parse_dt("2026-10-01T09:00").unwrap());

        let w = task("2026-10-02T09:00", None, Repeat::Weekdays); // a Friday
        let occ = occurrences(&w, parse_dt("2026-10-02").unwrap(), parse_dt("2026-10-09").unwrap());
        let days: Vec<_> = occ.iter().map(|o| o.start.day()).collect();
        assert_eq!(days, vec![2, 5, 6, 7, 8]);

        let far = occurrences(&w, parse_dt("2027-03-01").unwrap(), parse_dt("2027-03-08").unwrap());
        assert_eq!(far.len(), 5);
    }

    #[test]
    fn custom_intervals() {
        let t = task("2026-01-05T08:00", None, Repeat::Every(2, Unit::Week));
        let occ = occurrences(&t, parse_dt("2026-10-01").unwrap(), parse_dt("2026-11-01").unwrap());
        let days: Vec<_> = occ.iter().map(|o| o.start.date().to_string()).collect();
        assert_eq!(days, vec!["2026-10-12", "2026-10-26"]);
        let q = task("2026-01-31T09:00", None, Repeat::Every(3, Unit::Month));
        assert_eq!(next_start(&q), parse_dt("2026-04-30T09:00"));
        assert_eq!(Repeat::parse("every:2:week"), Repeat::Every(2, Unit::Week));
        assert_eq!(Repeat::parse("every:1:day"), Repeat::Daily);
        assert_eq!(Repeat::Every(2, Unit::Week).encode(), "every:2:week");
        assert_eq!(Repeat::Every(3, Unit::Day).label(), "Every 3 days");
        assert_eq!(Repeat::parse("every:x:week"), Repeat::None);
    }

    #[test]
    fn monthly_clamps_and_next() {
        let t = task("2026-01-31T09:00", None, Repeat::Monthly);
        let occ = occurrences(&t, parse_dt("2026-02-01").unwrap(), parse_dt("2026-03-01").unwrap());
        assert_eq!(occ[0].start, parse_dt("2026-02-28T09:00").unwrap());
        assert_eq!(next_start(&t), parse_dt("2026-02-28T09:00"));
    }
}
