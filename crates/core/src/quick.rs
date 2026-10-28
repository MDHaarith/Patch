//! Natural-language quick add: "Dentist tomorrow 3pm #work !!" becomes a
//! task titled "Dentist" tomorrow at 15:00 in the Work list with medium
//! priority.

use crate::model::{List, Repeat};
use crate::time::{add_months, hm, month_start, week_start};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuickTask {
    pub title: String,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    pub all_day: bool,
    pub list: Option<String>,
    pub priority: u8,
    pub repeat: Repeat,
}

const CONNECTORS: [&str; 7] = ["on", "at", "by", "due", "from", "starting", "@"];
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn month_of(tok: &str) -> Option<u32> {
    let t = tok.trim_end_matches(['.', ',']);
    if t.len() < 3 {
        return None;
    }
    let full = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    MONTHS
        .iter()
        .zip(full)
        .position(|(short, long)| long.starts_with(t) && t.starts_with(short) || t == "sept" && *short == "sep")
        .map(|i| i as u32 + 1)
}

fn weekday_of(tok: &str) -> Option<Weekday> {
    let t = tok.trim_end_matches([',', '.']);
    let days = [
        ("mon", "monday", Weekday::Mon),
        ("tue", "tuesday", Weekday::Tue),
        ("wed", "wednesday", Weekday::Wed),
        ("thu", "thursday", Weekday::Thu),
        ("fri", "friday", Weekday::Fri),
        ("sat", "saturday", Weekday::Sat),
        ("sun", "sunday", Weekday::Sun),
    ];
    days.iter()
        .find(|(short, long, _)| t.len() >= 3 && t.starts_with(short) && long.starts_with(t) || (*short == "tue" && t == "tues") || (*short == "thu" && (t == "thur" || t == "thurs")))
        .map(|(_, _, w)| *w)
}

fn day_number(tok: &str) -> Option<u32> {
    let t = tok.trim_end_matches([',', '.']);
    let t = t
        .strip_suffix("st")
        .or_else(|| t.strip_suffix("nd"))
        .or_else(|| t.strip_suffix("rd"))
        .or_else(|| t.strip_suffix("th"))
        .unwrap_or(t);
    let n: u32 = t.parse().ok()?;
    (1..=31).contains(&n).then_some(n)
}

/// Parses "3pm", "3:30pm", "15:00", "9a". `bare_ok` accepts "9" (after "at").
fn time_of(tok: &str, bare_ok: bool) -> Option<(u32, u32, Option<bool>)> {
    let t = tok.trim_end_matches([',', '.']);
    let (num, ampm) = if let Some(n) = t.strip_suffix("am").or_else(|| t.strip_suffix('a')) {
        (n, Some(false))
    } else if let Some(n) = t.strip_suffix("pm").or_else(|| t.strip_suffix('p')) {
        (n, Some(true))
    } else {
        (t, None)
    };
    let (h, m, colon) = match num.split_once([':', '.']) {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?, true),
        Some(_) => return None,
        None => (num.parse::<u32>().ok()?, 0, false),
    };
    if num.is_empty() || m > 59 {
        return None;
    }
    match ampm {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            let h = (h % 12) + if pm { 12 } else { 0 };
            Some((h, m, Some(pm)))
        }
        None if colon || bare_ok => (h <= 23).then_some((h, m, None)),
        None => None,
    }
}

fn apply_ampm(h: u32, pm: Option<bool>) -> u32 {
    match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    }
}

/// "10-11am", "3pm-5pm", "10:00-11:30"
fn time_range_of(tok: &str) -> Option<(NaiveTime, NaiveTime)> {
    let (a, b) = tok.split_once(['-', '–'])?;
    let (bh, bm, bpm) = time_of(b, true)?;
    let (ah, am, apm) = time_of(a, true)?;
    if apm.is_none() && bpm.is_none() && !(a.contains(':') || b.contains(':')) {
        return None;
    }
    let ah = if apm.is_none() {
        let candidate = apply_ampm(ah, bpm);
        // "11-1pm" means 11am to 1pm.
        if candidate * 60 + am > bh * 60 + bm && bpm == Some(true) { ah } else { candidate }
    } else {
        ah
    };
    Some((hm(ah, am), hm(bh, bm)))
}

fn duration_of(tok: &str) -> Option<Duration> {
    let t = tok.trim_end_matches([',', '.']);
    let mut total = 0i64;
    let mut num = String::new();
    let mut any = false;
    let mut rest = t;
    while !rest.is_empty() {
        let c = rest.chars().next()?;
        if c.is_ascii_digit() {
            num.push(c);
            rest = &rest[1..];
            continue;
        }
        let n: i64 = num.parse().ok()?;
        num.clear();
        let unit_len = rest.find(|ch: char| ch.is_ascii_digit()).unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        total = total.checked_add(match unit {
            "h" | "hr" | "hrs" | "hour" | "hours" => n.checked_mul(60)?,
            "m" | "min" | "mins" | "minute" | "minutes" => n,
            _ => return None,
        })?;
        any = true;
    }
    if !num.is_empty() || !any || total == 0 {
        return None;
    }
    Duration::try_minutes(total)
}

struct Parser<'a> {
    toks: Vec<String>,
    used: Vec<bool>,
    now: NaiveDateTime,
    lists: &'a [List],
    date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    time: Option<NaiveTime>,
    end_time: Option<NaiveTime>,
    duration: Option<Duration>,
    out: QuickTask,
}

impl Parser<'_> {
    fn tok(&self, i: usize) -> Option<&str> {
        self.toks.get(i).filter(|_| !self.used[i]).map(String::as_str)
    }

    fn take(&mut self, from: usize, n: usize) {
        for i in from..from + n {
            self.used[i] = true;
        }
        if from > 0 && !self.used[from - 1] && CONNECTORS.contains(&self.toks[from - 1].as_str()) {
            self.used[from - 1] = true;
        }
    }

    fn today(&self) -> NaiveDate {
        self.now.date()
    }

    /// Resolve day/month without a year to the next such date.
    fn upcoming(&self, month: u32, day: u32, year: Option<i32>) -> Option<NaiveDate> {
        if let Some(year) = year {
            return NaiveDate::from_ymd_opt(year, month, day);
        }
        // Eight years covers Feb 29 even across a non-leap century.
        (self.today().year()..=self.today().year() + 8)
            .filter_map(|y| NaiveDate::from_ymd_opt(y, month, day))
            .find(|d| *d >= self.today())
    }

    fn year_at(&self, i: usize) -> Option<i32> {
        let y: i32 = self.tok(i)?.trim_end_matches([',', '.']).parse().ok()?;
        (1970..=2200).contains(&y).then_some(y)
    }

    fn try_date(&mut self, i: usize) -> bool {
        let Some(t) = self.tok(i).map(str::to_string) else { return false };
        let today = self.today();
        let simple = match t.as_str() {
            "today" | "tod" => Some(today),
            "tonight" => {
                self.time.get_or_insert(hm(20, 0));
                Some(today)
            }
            "tomorrow" | "tmr" | "tmrw" | "tom" => Some(today + Duration::days(1)),
            "weekend" => Some(next_weekday(today, Weekday::Sat, true)),
            _ => None,
        };
        if let Some(d) = simple {
            self.date = Some(d);
            self.take(i, 1);
            return true;
        }
        if t == "next" || t == "this" {
            let next = t == "next";
            match self.tok(i + 1) {
                Some("week") if next => {
                    self.date = Some(week_start(today, true) + Duration::weeks(1));
                    self.take(i, 2);
                    return true;
                }
                Some("month") if next => {
                    self.date = Some(add_months(month_start(today), 1));
                    self.take(i, 2);
                    return true;
                }
                Some("weekend") => {
                    let sat = next_weekday(today, Weekday::Sat, true);
                    self.date = Some(if next && sat - today < Duration::days(6) { sat + Duration::weeks(1) } else { sat });
                    self.take(i, 2);
                    return true;
                }
                Some(w) => {
                    if let Some(wd) = weekday_of(w) {
                        let d = next_weekday(today, wd, false);
                        self.date = Some(if next { d + Duration::weeks(1) } else { d });
                        self.take(i, 2);
                        return true;
                    }
                }
                None => {}
            }
        }
        if let Some(wd) = weekday_of(&t) {
            self.date = Some(next_weekday(today, wd, true));
            self.take(i, 1);
            return true;
        }
        if t == "in" {
            if let (Some(n), Some(unit)) = (self.tok(i + 1).and_then(|n| n.parse::<i64>().ok()), self.tok(i + 2)) {
                let unit = unit.trim_end_matches(['s', ',', '.']);
                let applied = match unit {
                    "day" => Duration::try_days(n).and_then(|d| today.checked_add_signed(d)).map(|d| (Some(d), None)),
                    "week" => Duration::try_weeks(n).and_then(|d| today.checked_add_signed(d)).map(|d| (Some(d), None)),
                    "month" => u32::try_from(n).ok().and_then(|n| today.checked_add_months(chrono::Months::new(n))).map(|d| (Some(d), None)),
                    "hour" | "hr" | "h" => Duration::try_hours(n).and_then(|d| self.now.checked_add_signed(d)).map(|at| (None, Some(at))),
                    "min" | "minute" | "m" => Duration::try_minutes(n).and_then(|d| self.now.checked_add_signed(d)).map(|at| (None, Some(at))),
                    _ => None,
                };
                if let Some((date, at)) = applied {
                    if let Some(at) = at {
                        self.date = Some(at.date());
                        self.time = Some(at.time());
                    } else {
                        self.date = date;
                    }
                    self.take(i, 3);
                    return true;
                }
            }
        }
        // ISO date
        if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
            self.date = Some(d);
            self.take(i, 1);
            return true;
        }
        // 13/10 or 13/10/2026 or 13.10 (day first)
        let parts: Vec<&str> = t.split(['/', '.']).collect();
        if (2..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && t.contains('/') {
            let day = parts[0].parse().ok();
            let month = parts[1].parse().ok();
            let year = parts.get(2).and_then(|y| y.parse::<i32>().ok()).map(|y| if y < 100 { 2000 + y } else { y });
            if let (Some(day), Some(month)) = (day, month) {
                if let Some(d) = self.upcoming(month, day, year) {
                    self.date = Some(d);
                    self.take(i, 1);
                    return true;
                }
            }
        }
        // "13 oct [2026]", "13-15 oct", "13th of october"
        let (first, range_end) = match t.split_once('-') {
            Some((a, b)) => (day_number(a), day_number(b)),
            None => (day_number(&t), None),
        };
        if let Some(day) = first {
            let mut j = i + 1;
            if self.tok(j) == Some("of") {
                j += 1;
            }
            if let Some(month) = self.tok(j).and_then(month_of) {
                let year = self.year_at(j + 1);
                if let Some(d) = self.upcoming(month, day, year) {
                    self.date = Some(d);
                    if let Some(end) = range_end.and_then(|e| NaiveDate::from_ymd_opt(d.year(), month, e)) {
                        self.end_date = Some(end.max(d));
                    }
                    self.take(i, j - i + 1 + year.is_some() as usize);
                    return true;
                }
            }
        }
        // "oct 13", "october 13-15 2026"
        if let Some(month) = month_of(&t) {
            if let Some(next) = self.tok(i + 1).map(str::to_string) {
                let (first, range_end) = match next.split_once('-') {
                    Some((a, b)) => (day_number(a), day_number(b)),
                    None => (day_number(&next), None),
                };
                if let Some(day) = first {
                    let year = self.year_at(i + 2);
                    if let Some(d) = self.upcoming(month, day, year) {
                        self.date = Some(d);
                        if let Some(end) = range_end.and_then(|e| NaiveDate::from_ymd_opt(d.year(), month, e)) {
                            self.end_date = Some(end.max(d));
                        }
                        self.take(i, 2 + year.is_some() as usize);
                        return true;
                    }
                }
            }
        }
        false
    }

    fn try_time(&mut self, i: usize) -> bool {
        let Some(t) = self.tok(i).map(str::to_string) else { return false };
        let after_at = i > 0 && self.toks[i - 1] == "at";
        let named = match t.as_str() {
            "noon" | "midday" => Some(hm(12, 0)),
            "midnight" => Some(hm(23, 59)),
            "morning" => Some(hm(9, 0)),
            "afternoon" => Some(hm(14, 0)),
            "evening" => Some(hm(18, 0)),
            _ => None,
        };
        if let Some(time) = named {
            self.time = Some(time);
            self.take(i, 1);
            return true;
        }
        if let Some((a, b)) = time_range_of(&t) {
            self.time = Some(a);
            self.end_time = Some(b);
            self.take(i, 1);
            return true;
        }
        // "3 pm"
        if let (Some(n), Some(suffix)) = (t.parse::<u32>().ok(), self.tok(i + 1)) {
            if matches!(suffix, "am" | "pm") {
                if let Some((h, m, _)) = time_of(&format!("{n}{suffix}"), false) {
                    self.time = Some(hm(h, m));
                    self.take(i, 2);
                    return true;
                }
            }
        }
        if let Some((h, m, ampm)) = time_of(&t, after_at) {
            // "at 5" with no am/pm and no minutes: assume office hours.
            let h = if ampm.is_none() && !t.contains(':') && (1..=7).contains(&h) { h + 12 } else { h };
            self.time = Some(hm(h, m));
            self.take(i, 1);
            // "3pm to 5pm" / "3pm - 5pm" / "3pm until 5"
            if let (Some(sep), Some(end)) = (self.tok(i + 1).map(str::to_string), self.tok(i + 2).map(str::to_string)) {
                if matches!(sep.as_str(), "to" | "-" | "until" | "till") {
                    if let Some((eh, em, epm)) = time_of(&end, true) {
                        let eh = if epm.is_none() && eh < h && eh < 12 { eh + 12 } else { eh };
                        self.end_time = Some(hm(eh, em));
                        self.used[i + 1] = true;
                        self.used[i + 2] = true;
                    }
                }
            }
            return true;
        }
        false
    }

    fn try_meta(&mut self, i: usize) -> bool {
        let Some(t) = self.tok(i).map(str::to_string) else { return false };
        if let Some(name) = t.strip_prefix('#') {
            let name = name.to_lowercase();
            if name.is_empty() {
                return false;
            }
            let hit = self
                .lists
                .iter()
                .find(|l| l.name.to_lowercase().replace(' ', "") == name)
                .or_else(|| self.lists.iter().find(|l| l.name.to_lowercase().replace(' ', "").starts_with(&name)));
            if let Some(list) = hit {
                self.out.list = Some(list.id.clone());
                self.used[i] = true;
                return true;
            }
            return false;
        }
        if let Some(p) = t.strip_prefix('!') {
            // More marks, higher priority: "!" low, "!!" medium, "!!!" high.
            let pri = match p {
                _ if p.chars().all(|c| c == '!') => Some((p.len() as u8 + 1).min(3)),
                "1" | "high" | "hi" => Some(3),
                "2" | "med" | "medium" => Some(2),
                "3" | "low" | "lo" => Some(1),
                _ => None,
            };
            if let Some(pri) = pri {
                self.out.priority = pri;
                self.used[i] = true;
                return true;
            }
        }
        let repeat = match t.as_str() {
            "daily" => Some((Repeat::Daily, 1)),
            "weekdays" => Some((Repeat::Weekdays, 1)),
            "weekly" => Some((Repeat::Weekly, 1)),
            "monthly" => Some((Repeat::Monthly, 1)),
            "yearly" | "annually" => Some((Repeat::Yearly, 1)),
            "every" => match self.tok(i + 1) {
                Some("day") => Some((Repeat::Daily, 2)),
                Some("weekday") | Some("weekdays") => Some((Repeat::Weekdays, 2)),
                Some("week") => Some((Repeat::Weekly, 2)),
                Some("month") => Some((Repeat::Monthly, 2)),
                Some("year") => Some((Repeat::Yearly, 2)),
                Some(w) => weekday_of(w).map(|wd| {
                    self.date.get_or_insert(next_weekday(self.now.date(), wd, true));
                    (Repeat::Weekly, 2)
                }),
                None => None,
            },
            _ => None,
        };
        if let Some((rep, n)) = repeat {
            self.out.repeat = rep;
            self.take(i, n);
            return true;
        }
        if t == "for" {
            if let Some(d) = self.tok(i + 1).and_then(duration_of).filter(|d| self.now.checked_add_signed(*d).is_some()) {
                self.duration = Some(d);
                self.take(i, 2);
                return true;
            }
            if let (Some(n), Some(unit)) = (self.tok(i + 1).and_then(|n| n.parse::<i64>().ok()), self.tok(i + 2)) {
                if let Some(d) = duration_of(&format!("{n}{unit}")).filter(|d| self.now.checked_add_signed(*d).is_some()) {
                    self.duration = Some(d);
                    self.take(i, 3);
                    return true;
                }
            }
        }
        false
    }
}

/// Next date with weekday `wd`, today included when `include_today`.
fn next_weekday(from: NaiveDate, wd: Weekday, include_today: bool) -> NaiveDate {
    let mut diff = (wd.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64).rem_euclid(7);
    if diff == 0 && !include_today {
        diff = 7;
    }
    from + Duration::days(diff)
}

pub fn parse(input: &str, now: NaiveDateTime, lists: &[List]) -> QuickTask {
    let raw: Vec<&str> = input.split_whitespace().collect();
    let toks: Vec<String> = raw.iter().map(|t| t.to_lowercase()).collect();
    let mut p = Parser {
        used: vec![false; toks.len()],
        toks,
        now,
        lists,
        date: None,
        end_date: None,
        time: None,
        end_time: None,
        duration: None,
        out: QuickTask::default(),
    };
    for i in 0..p.toks.len() {
        if p.used[i] {
            continue;
        }
        let _ = p.try_meta(i) || (p.date.is_none() && p.try_date(i)) || (p.time.is_none() && p.try_time(i));
    }

    let title: Vec<&str> = raw.iter().zip(&p.used).filter(|(_, u)| !**u).map(|(t, _)| *t).collect();
    let mut out = p.out;
    out.title = title.join(" ").trim_matches([',', '-', ' ']).to_string();

    match (p.date, p.time) {
        (date, Some(time)) => {
            let mut day = date.unwrap_or(now.date());
            if date.is_none() && day.and_time(time) < now - Duration::minutes(1) {
                day += Duration::days(1);
            }
            let start = day.and_time(time);
            out.start = Some(start);
            out.end = match (p.end_time, p.duration) {
                (Some(et), _) => {
                    let end_day = p.end_date.unwrap_or(day);
                    let mut end = end_day.and_time(et);
                    if end <= start {
                        end += Duration::days(1);
                    }
                    Some(end)
                }
                (None, Some(d)) => start.checked_add_signed(d),
                (None, None) => p.end_date.map(|d| d.and_time(time)).filter(|e| *e > start),
            };
        }
        (Some(date), None) => {
            out.all_day = true;
            out.start = Some(date.and_time(NaiveTime::MIN));
            out.end = p.end_date.map(|d| d.and_time(NaiveTime::MIN));
        }
        (None, None) if out.repeat != Repeat::None => {
            out.all_day = true;
            out.start = Some(now.date().and_time(NaiveTime::MIN));
        }
        (None, None) => {}
    }
    if out.title.is_empty() {
        out.title = input.trim().to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_dt;

    fn now() -> NaiveDateTime {
        // Thursday 22 October 2026, 09:30
        parse_dt("2026-10-22T09:30").unwrap()
    }

    fn lists() -> Vec<List> {
        vec![
            List { id: "l1".into(), name: "Personal".into(), color: "#fff".into(), order: 1.0 },
            List { id: "l2".into(), name: "Work".into(), color: "#fff".into(), order: 2.0 },
        ]
    }

    fn q(s: &str) -> QuickTask {
        parse(s, now(), &lists())
    }

    #[test]
    fn plain_title() {
        let t = q("Buy milk");
        assert_eq!(t.title, "Buy milk");
        assert!(t.start.is_none());
    }

    #[test]
    fn tomorrow_at_time_with_list_and_priority() {
        assert_eq!(q("x !!").priority, 2);
        let t = q("Dentist tomorrow at 3pm #work !!!");
        assert_eq!(t.title, "Dentist");
        assert_eq!(t.start, parse_dt("2026-10-23T15:00"));
        assert_eq!(t.list.as_deref(), Some("l2"));
        assert_eq!(t.priority, 3);
        assert!(!t.all_day);
    }

    #[test]
    fn weekday_and_range() {
        let t = q("Team sync fri 10-11am");
        assert_eq!(t.title, "Team sync");
        assert_eq!(t.start, parse_dt("2026-10-23T10:00"));
        assert_eq!(t.end, parse_dt("2026-10-23T11:00"));

        let t = q("Workshop next monday 2pm to 5pm");
        assert_eq!(t.start, parse_dt("2026-11-02T14:00"));
        assert_eq!(t.end, parse_dt("2026-11-02T17:00"));
    }

    #[test]
    fn date_range_all_day() {
        let t = q("Conference 13-15 nov");
        assert_eq!(t.title, "Conference");
        assert!(t.all_day);
        assert_eq!(t.start, parse_dt("2026-11-13"));
        assert_eq!(t.end, parse_dt("2026-11-15"));

        let t = q("Trip oct 13 2027");
        assert_eq!(t.start, parse_dt("2027-10-13"));
    }

    #[test]
    fn past_dates_roll_to_next_year() {
        let t = q("Birthday 13/10");
        assert_eq!(t.start, parse_dt("2027-10-13"));
    }

    #[test]
    fn time_only_rolls_to_tomorrow_when_passed() {
        assert_eq!(q("Call mom 8am").start, parse_dt("2026-10-23T08:00"));
        assert_eq!(q("Call mom 18:30").start, parse_dt("2026-10-22T18:30"));
    }

    #[test]
    fn durations_and_repeats() {
        let t = q("Gym tomorrow 7am for 1h30m");
        assert_eq!(t.end, parse_dt("2026-10-23T08:30"));
        let t = q("Standup every weekday 9:45");
        assert_eq!(t.title, "Standup");
        assert_eq!(t.repeat, Repeat::Weekdays);
        assert_eq!(t.start, parse_dt("2026-10-22T09:45"));
        let t = q("Water plants every sunday");
        assert_eq!(t.repeat, Repeat::Weekly);
        assert_eq!(t.start, parse_dt("2026-10-25"));
    }

    #[test]
    fn in_n_units() {
        assert_eq!(q("Follow up in 3 days").start, parse_dt("2026-10-25"));
        assert_eq!(q("Check oven in 2 hours").start, parse_dt("2026-10-22T11:30"));
    }

    #[test]
    fn overflowing_dates_and_durations_are_unparsed() {
        for input in [
            "Call in 100000000 days", "Call in 9223372036854775807 weeks",
            "Call in 9223372036854775807 hours", "Call in 9223372036854775807 minutes",
            "Call in 4294967296 months",
        ] {
            let task = q(input);
            assert_eq!(task.start, None, "{input}");
            assert_eq!(task.title, input);
        }
        for duration in ["9223372036854775807m", "9223372036854775807h", "9223372036854775800m8m"] {
            assert!(duration_of(duration).is_none());
            let task = q(&format!("Call 3pm for {duration}"));
            assert_eq!(task.end, None);
            assert!(task.title.contains(duration));
        }
        let task = q("Call 3pm for 1000000000000m");
        assert_eq!(task.end, None);
        assert!(task.title.contains("1000000000000m"));
    }

    #[test]
    fn leap_day_finds_next_valid_year() {
        assert_eq!(q("Birthday 29 feb").start, parse_dt("2028-02-29"));
        assert_eq!(q("Birthday feb 29").start, parse_dt("2028-02-29"));
        assert_eq!(q("Birthday 29/2").start, parse_dt("2028-02-29"));
        assert_eq!(q("Birthday 29 feb 2027").start, None);
        assert_eq!(q("Birthday 30 feb").start, None);
        let after_leap_day = parse_dt("2096-03-01").unwrap();
        assert_eq!(parse("Birthday 29 feb", after_leap_day, &[]).start, parse_dt("2104-02-29"));
    }

    #[test]
    fn unknown_hashtag_is_kept() {
        let t = q("Read #rust book");
        assert_eq!(t.title, "Read #rust book");
        assert_eq!(t.list, None);
    }
}
