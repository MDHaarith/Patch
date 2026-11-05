//! iCalendar (RFC 5545): the hub's read-only feed and calendar file import.
//!
//! Patch stores floating local times, so exported times are floating too.

use crate::doc::{Doc, COL_TODO, DEFAULT_LIST};
use crate::model::{Repeat, Task, Unit};
use crate::recur;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday};

/// A calendar of every open, scheduled task.
pub fn export(tasks: &[Task]) -> String {
    let mut out = String::new();
    for line in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Patch//Patch//EN", "CALSCALE:GREGORIAN", "X-WR-CALNAME:Patch"] {
        push(&mut out, line);
    }
    for task in tasks.iter().filter(|t| !t.done && t.active()) {
        if let (Some(start), Some(end)) = (task.start, task.span_end()) {
            push_event(&mut out, task, start, end);
        }
    }
    push(&mut out, "END:VCALENDAR");
    out
}

fn push_event(out: &mut String, task: &Task, start: NaiveDateTime, end: NaiveDateTime) {
    let stamp = DateTime::<Utc>::from_timestamp_millis(task.created).unwrap_or_default();
    push(out, "BEGIN:VEVENT");
    push(out, &format!("UID:{}@patch", escape(&task.id)));
    push(out, &format!("DTSTAMP:{}", stamp.format("%Y%m%dT%H%M%SZ")));
    if task.all_day {
        push(out, &format!("DTSTART;VALUE=DATE:{}", start.format("%Y%m%d")));
        push(out, &format!("DTEND;VALUE=DATE:{}", end.format("%Y%m%d")));
    } else {
        push(out, &format!("DTSTART:{}", start.format("%Y%m%dT%H%M%S")));
        push(out, &format!("DTEND:{}", end.format("%Y%m%dT%H%M%S")));
    }
    push(out, &format!("SUMMARY:{}", escape(&task.title)));
    if !task.notes.is_empty() {
        push(out, &format!("DESCRIPTION:{}", escape(&task.notes)));
    }
    if let Some(rule) = rrule(task.repeat, task.repeat_anchor.unwrap_or(start)) {
        push(out, &format!("RRULE:{rule}"));
    }
    for minutes in task.reminder_offsets().into_iter().filter(|m| *m >= 0) {
        push(out, "BEGIN:VALARM");
        push(out, "ACTION:DISPLAY");
        push(out, &format!("DESCRIPTION:{}", escape(&task.title)));
        push(out, &format!("TRIGGER:-PT{minutes}M"));
        push(out, "END:VALARM");
    }
    push(out, "END:VEVENT");
}

fn rrule(repeat: Repeat, anchor: NaiveDateTime) -> Option<String> {
    // Patch clamps "the 31st" to short months; plain RFC rules would skip them.
    let clamp = || {
        let day = anchor.day();
        (day > 28).then(|| format!(";BYMONTHDAY={};BYSETPOS=-1", (28..=day).map(|d| d.to_string()).collect::<Vec<_>>().join(",")))
    };
    let monthly = |n: u32| format!("FREQ=MONTHLY;INTERVAL={n}{}", clamp().unwrap_or_default());
    let yearly = |n: u32| match clamp() {
        Some(days) => format!("FREQ=YEARLY;INTERVAL={n};BYMONTH={}{days}", anchor.month()),
        None => format!("FREQ=YEARLY;INTERVAL={n}"),
    };
    Some(match repeat {
        // shortcut: RFC 5545 can't express repeat-after-completion, so the feed shows only the next date.
        Repeat::None | Repeat::AfterDone(..) => return None,
        Repeat::Daily => "FREQ=DAILY".into(),
        Repeat::Weekdays => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into(),
        Repeat::Weekly => "FREQ=WEEKLY".into(),
        Repeat::Monthly => monthly(1),
        Repeat::Yearly => yearly(1),
        Repeat::Every(n, Unit::Day) => format!("FREQ=DAILY;INTERVAL={n}"),
        Repeat::Every(n, Unit::Week) => format!("FREQ=WEEKLY;INTERVAL={n}"),
        Repeat::Every(n, Unit::Month) => monthly(n as u32),
        Repeat::Every(n, Unit::Year) => yearly(n as u32),
    })
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\r', "").replace('\n', "\\n")
}

/// Appends a content line, folded at 75 octets without splitting a character.
fn push(out: &mut String, line: &str) {
    let mut octets = 0;
    for c in line.chars() {
        if octets + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            octets = 1;
        }
        out.push(c);
        octets += c.len_utf8();
    }
    out.push_str("\r\n");
}

/// What an import did, for telling the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    /// Imported before (or deleted since), cancelled, or unreadable.
    pub skipped: usize,
    /// Added, but a repeat rule or its exceptions couldn't be represented.
    pub simplified: usize,
    /// Added at the wall-clock time written in the event's own time zone.
    pub zoned: usize,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        let plural = if self.added == 1 { "" } else { "s" };
        let mut out = format!("Imported {} event{plural}", self.added);
        if self.skipped > 0 {
            out += &format!(", skipped {}", self.skipped);
        }
        if self.simplified > 0 {
            out += &format!(", {} with a simplified repeat", self.simplified);
        }
        if self.zoned > 0 {
            out += &format!(", {} kept the time written in their own time zone", self.zoned);
        }
        out
    }
}

pub fn is_calendar(text: &str) -> bool {
    text.lines().any(|l| l.trim().eq_ignore_ascii_case("BEGIN:VCALENDAR"))
}

/// Adds a calendar's events as tasks. Ids come from event UIDs, so importing a
/// file twice, or on two devices, can't duplicate anything and tasks you've
/// deleted since stay deleted.
pub fn import(doc: &mut Doc, text: &str, now: NaiveDateTime) -> ImportReport {
    let mut report = ImportReport::default();
    let mut seen = std::collections::HashSet::new();
    let mut tasks = Vec::new();
    for event in events(text) {
        let id = event.id();
        if doc.record(&id).is_some() || !seen.insert(id.clone()) {
            report.skipped += 1;
            continue;
        }
        let Some(mut task) = event.task(&id, &mut report) else {
            report.skipped += 1;
            continue;
        };
        // A past one-off is history, not overdue work; a past repeat resumes at its next occurrence.
        task.done = task.span_end().is_some_and(|end| end <= now) && !advance(&mut task, now);
        tasks.push(task);
    }
    report.added = tasks.len();
    doc.add_tasks(tasks);
    report
}

fn advance(task: &mut Task, now: NaiveDateTime) -> bool {
    let Some(start) = task.start.filter(|_| task.repeat != Repeat::None) else { return false };
    let horizon = now + Duration::days(36_600);
    let Some(next) = recur::occurrences(task, now, horizon).first().map(|o| o.start) else { return false };
    task.repeat_anchor = Some(start);
    task.start = Some(next);
    task.end = task.end.map(|e| e + (next - start));
    true
}

#[derive(Default)]
struct Event {
    props: Vec<(String, String, String)>,
    alarm: Option<i64>,
}

impl Event {
    fn prop(&self, name: &str) -> Option<(&str, &str)> {
        self.props.iter().find(|(n, _, _)| n == name).map(|(_, params, value)| (params.as_str(), value.as_str()))
    }

    fn id(&self) -> String {
        let key = match self.prop("UID") {
            Some((_, uid)) => uid.to_string(),
            None => format!("{}-{}", self.prop("SUMMARY").map_or("", |p| p.1), self.prop("DTSTART").map_or("", |p| p.1)),
        };
        let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        format!("ics-{safe}")
    }

    fn task(&self, id: &str, report: &mut ImportReport) -> Option<Task> {
        // shortcut: edited instances of a repeat (RECURRENCE-ID) are dropped, handle them if people import shared calendars.
        if self.prop("RECURRENCE-ID").is_some() || self.prop("STATUS").is_some_and(|s| s.1.eq_ignore_ascii_case("CANCELLED")) {
            return None;
        }
        let (start_params, start_value) = self.prop("DTSTART")?;
        let (start, all_day) = moment(start_params, start_value)?;
        let mut task = Task::blank(id, DEFAULT_LIST, COL_TODO);
        task.title = self.prop("SUMMARY").map(|s| unescape(s.1)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Untitled event".into());
        task.notes = self.prop("DESCRIPTION").map(|d| unescape(d.1)).unwrap_or_default();
        task.start = Some(start);
        task.all_day = all_day;
        let end = match self.prop("DTEND") {
            Some((params, value)) => moment(params, value).map(|m| m.0),
            None => self.prop("DURATION").and_then(|d| duration(d.1)).map(|d| start + d),
        };
        task.end = end.filter(|e| *e > start).map(|e| if all_day { e - Duration::days(1) } else { e });
        task.reminder = self.alarm;
        if let Some((_, rule)) = self.prop("RRULE") {
            match repeat(rule, start) {
                Some(rule) => task.repeat = rule,
                None => report.simplified += 1,
            }
        }
        if task.repeat != Repeat::None && self.prop("EXDATE").is_some() {
            report.simplified += 1;
        }
        // shortcut: without a time zone database, a TZID time keeps its wall clock; add chrono-tz if imports across zones matter.
        if !all_day && start_params.to_ascii_uppercase().contains("TZID=") {
            report.zoned += 1;
        }
        Some(task)
    }
}

fn events(text: &str) -> Vec<Event> {
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in normalized.split('\n') {
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }
    let mut out = Vec::new();
    let mut current: Option<Event> = None;
    let mut nested: Vec<String> = Vec::new();
    for line in &lines {
        let Some((name, params, value)) = content_line(line) else { continue };
        match (name.as_str(), &mut current) {
            ("BEGIN", None) if value.eq_ignore_ascii_case("VEVENT") => current = Some(Event::default()),
            ("BEGIN", Some(_)) => nested.push(value.to_ascii_uppercase()),
            ("END", Some(_)) if !nested.is_empty() => {
                nested.pop();
            }
            ("END", Some(_)) if value.eq_ignore_ascii_case("VEVENT") => out.extend(current.take()),
            ("TRIGGER", Some(event)) if nested.last().is_some_and(|c| c == "VALARM") && event.alarm.is_none() => {
                let before = duration(value).map(|d| -d.num_minutes()).filter(|m| *m >= 0);
                event.alarm = before.filter(|_| !params.to_ascii_uppercase().contains("RELATED=END"));
            }
            (_, Some(event)) if nested.is_empty() => event.props.push((name, params.to_string(), value.to_string())),
            _ => {}
        }
    }
    out
}

/// Splits `NAME;PARAM="a:b":value`; a colon inside quotes belongs to the parameter.
fn content_line(line: &str) -> Option<(String, &str, &str)> {
    let mut quoted = false;
    let colon = line.char_indices().find(|(_, c)| {
        if *c == '"' {
            quoted = !quoted;
        }
        *c == ':' && !quoted
    })?.0;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let (name, params) = head.split_once(';').unwrap_or((head, ""));
    Some((name.trim().to_ascii_uppercase(), params, value.trim_end()))
}

fn moment(params: &str, value: &str) -> Option<(NaiveDateTime, bool)> {
    if params.to_ascii_uppercase().contains("VALUE=DATE") && !params.to_ascii_uppercase().contains("VALUE=DATE-TIME") || value.len() == 8 {
        return Some((NaiveDate::parse_from_str(value, "%Y%m%d").ok()?.and_time(NaiveTime::MIN), true));
    }
    if let Some(utc) = value.strip_suffix(['Z', 'z']) {
        let utc = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((DateTime::<Utc>::from_naive_utc_and_offset(utc, Utc).with_timezone(&Local).naive_local(), false));
    }
    Some((NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?, false))
}

/// `-P1DT2H30M`, `PT15M`, `P1W`.
fn duration(value: &str) -> Option<Duration> {
    let (sign, rest) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let mut total = 0i64;
    let mut number = String::new();
    for c in rest.strip_prefix('P')?.chars() {
        match c {
            '0'..='9' => number.push(c),
            'T' if number.is_empty() => {}
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                let unit = match c {
                    'W' => 604_800,
                    'D' => 86_400,
                    'H' => 3_600,
                    'M' => 60,
                    _ => 1,
                };
                total = total.checked_add(n.checked_mul(unit)?)?;
            }
            _ => return None,
        }
    }
    number.is_empty().then(|| Duration::try_seconds(sign * total)).flatten()
}

/// The Patch repeat that means exactly `rule`, or `None` when there isn't one
/// (COUNT, UNTIL, "second Tuesday"...): a dropped repeat beats invented dates.
fn repeat(rule: &str, start: NaiveDateTime) -> Option<Repeat> {
    let (mut freq, mut interval, mut by_day, mut by_month, mut by_month_day, mut by_set_pos) = (None, 1u16, None, None, None, None);
    for part in rule.split(';').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=')?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(value.to_ascii_uppercase()),
            "INTERVAL" => interval = value.parse().ok().filter(|n| *n > 0)?,
            "WKST" => {}
            "BYDAY" => by_day = Some(value.to_ascii_uppercase()),
            "BYMONTH" => by_month = Some(value),
            "BYMONTHDAY" => by_month_day = Some(value),
            "BYSETPOS" => by_set_pos = Some(value),
            _ => return None,
        }
    }
    let day = start.day().to_string();
    // Either the start's own day, or the "last of 28..31" form export() writes.
    let on_own_day = match (by_month_day, by_set_pos) {
        (None, None) => true,
        (Some(days), None) => days == day,
        (Some(days), Some("-1")) => days.starts_with("28") && days.split(',').next_back() == Some(day.as_str()),
        _ => false,
    };
    let every = |unit: &str| Some(Repeat::parse(&format!("every:{interval}:{unit}")));
    match freq?.as_str() {
        "DAILY" if by_day.is_none() && by_month.is_none() && by_month_day.is_none() && by_set_pos.is_none() => every("day"),
        "WEEKLY" if by_month.is_none() && by_month_day.is_none() && by_set_pos.is_none() => match by_day.as_deref() {
            None => every("week"),
            Some(days) if days == weekday_code(start.weekday()) => every("week"),
            Some(days) if interval == 1 && same_days(days, "MO,TU,WE,TH,FR") => Some(Repeat::Weekdays),
            _ => None,
        },
        "MONTHLY" if by_day.is_none() && by_month.is_none() && on_own_day => every("month"),
        "YEARLY" if by_day.is_none() && on_own_day && by_month.is_none_or(|m| m == start.month().to_string()) => every("year"),
        _ => None,
    }
}

fn same_days(a: &str, b: &str) -> bool {
    let sorted = |s: &str| {
        let mut days: Vec<String> = s.split(',').map(str::to_string).collect();
        days.sort();
        days
    };
    sorted(a) == sorted(b)
}

fn weekday_code(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            },
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_dt;

    fn now() -> NaiveDateTime {
        parse_dt("2026-11-04T12:00").unwrap()
    }

    fn calendar(body: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{body}END:VCALENDAR\r\n")
    }

    fn event(body: &str) -> String {
        calendar(&format!("BEGIN:VEVENT\r\n{body}END:VEVENT\r\n"))
    }

    fn imported(body: &str) -> Task {
        let mut doc = Doc::default();
        assert_eq!(import(&mut doc, &event(&format!("UID:x\r\n{body}")), now()).added, 1);
        doc.task("ics-x").unwrap()
    }

    fn task(title: &str, start: &str, end: Option<&str>) -> Task {
        let mut t = Task::blank("t-1", DEFAULT_LIST, COL_TODO);
        t.title = title.into();
        t.start = parse_dt(start);
        t.end = end.and_then(parse_dt);
        t
    }

    #[test]
    fn exports_timed_and_all_day_events() {
        let timed = task("Dentist, again; \\ok\nsecond", "2026-11-05T15:00", Some("2026-11-05T15:30"));
        let mut day = task("Trip", "2026-11-13", Some("2026-11-15"));
        day.all_day = true;
        let ics = export(&[timed, day]);
        assert!(ics.contains("DTSTART:20261105T150000\r\nDTEND:20261105T153000\r\n"));
        assert!(ics.contains("SUMMARY:Dentist\\, again\\; \\\\ok\\nsecond\r\n"));
        // iCalendar's end is exclusive, Patch's all-day end is the last day.
        assert!(ics.contains("DTSTART;VALUE=DATE:20261113\r\nDTEND;VALUE=DATE:20261116\r\n"));
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n") && ics.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn export_skips_done_and_unscheduled_tasks() {
        let mut done = task("Done", "2026-11-05T15:00", None);
        done.done = true;
        let mut someday = task("Someday", "2026-11-05", None);
        someday.start = None;
        assert!(!export(&[done, someday]).contains("BEGIN:VEVENT"));
    }

    #[test]
    fn long_lines_fold_without_splitting_characters() {
        let ics = export(&[task(&"日本語のタイトル".repeat(12), "2026-11-05T15:00", None)]);
        assert!(ics.split("\r\n").all(|line| line.len() <= 75));
        let again = imported_from(&ics);
        assert_eq!(again.title, "日本語のタイトル".repeat(12));
    }

    fn imported_from(ics: &str) -> Task {
        let mut doc = Doc::default();
        assert_eq!(import(&mut doc, ics, now()).added, 1);
        doc.tasks().into_iter().find(|t| t.id.starts_with("ics-")).unwrap()
    }

    #[test]
    fn repeats_survive_a_round_trip() {
        for (repeat, start) in [
            (Repeat::Daily, "2026-12-01T09:00"),
            (Repeat::Weekdays, "2026-12-01T09:00"),
            (Repeat::Weekly, "2026-12-01T09:00"),
            (Repeat::Monthly, "2026-12-01T09:00"),
            (Repeat::Monthly, "2026-12-31T09:00"),
            (Repeat::Yearly, "2027-02-28T09:00"),
            (Repeat::Yearly, "2028-02-29T09:00"),
            (Repeat::Every(2, Unit::Week), "2026-12-01T09:00"),
            (Repeat::Every(3, Unit::Day), "2026-12-01T09:00"),
            (Repeat::Every(3, Unit::Month), "2026-12-31T09:00"),
            (Repeat::Every(2, Unit::Year), "2026-12-01T09:00"),
        ] {
            let mut t = task("Repeat", start, None);
            t.repeat = repeat;
            let again = imported_from(&export(&[t]));
            assert_eq!(again.repeat, repeat, "{start}");
            assert_eq!(again.start, parse_dt(start));
        }
    }

    #[test]
    fn monthly_rule_keeps_patch_clamping() {
        let mut t = task("Rent", "2027-01-31T09:00", None);
        t.repeat = Repeat::Monthly;
        assert!(export(&[t]).contains("RRULE:FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=28,29,30,31;BYSETPOS=-1\r\n"));
    }

    #[test]
    fn reminders_round_trip() {
        let mut t = task("Call", "2026-12-01T09:00", None);
        t.reminder = Some(90);
        assert_eq!(imported_from(&export(&[t])).reminder, Some(90));
        let t = imported("DTSTART:20261201T090000\r\nBEGIN:VALARM\r\nTRIGGER:-P1DT1H\r\nDESCRIPTION:not the event\r\nEND:VALARM\r\nSUMMARY:Real\r\n");
        assert_eq!(t.reminder, Some(1500));
        assert_eq!(t.title, "Real");
        assert_eq!(t.notes, "");
    }

    #[test]
    fn imports_folded_quoted_and_all_day_properties() {
        let t = imported("SUMMARY:Long \r\n title\\, here\r\nDTSTART;TZID=\"America/New_York:odd\":20261201T090000\r\nDURATION:PT45M\r\n");
        assert_eq!(t.title, "Long title, here");
        assert_eq!(t.start, parse_dt("2026-12-01T09:00"));
        assert_eq!(t.end, parse_dt("2026-12-01T09:45"));

        let t = imported("SUMMARY:Trip\r\nDTSTART;VALUE=DATE:20261213\r\nDTEND;VALUE=DATE:20261216\r\n");
        assert!(t.all_day);
        assert_eq!(t.end, parse_dt("2026-12-15"));
        let t = imported("SUMMARY:One day\r\nDTSTART;VALUE=DATE:20261213\r\nDTEND;VALUE=DATE:20261214\r\n");
        assert_eq!(t.span_end(), parse_dt("2026-12-14"));
    }

    #[test]
    fn utc_times_become_local_and_zoned_times_are_reported() {
        let t = imported("SUMMARY:UTC\r\nDTSTART:20261201T090000Z\r\n");
        let utc = parse_dt("2026-12-01T09:00").unwrap();
        assert_eq!(t.start, Some(DateTime::<Utc>::from_naive_utc_and_offset(utc, Utc).with_timezone(&Local).naive_local()));

        let mut doc = Doc::default();
        let report = import(&mut doc, &event("UID:z\r\nDTSTART;TZID=Asia/Tokyo:20261201T090000\r\n"), now());
        assert_eq!(report.zoned, 1);
    }

    #[test]
    fn importing_twice_adds_nothing_and_deleted_tasks_stay_deleted() {
        let ics = event("UID:same@example.com\r\nSUMMARY:Once\r\nDTSTART:20261201T090000\r\n");
        let mut doc = Doc::default();
        assert_eq!(import(&mut doc, &ics, now()).added, 1);
        let again = import(&mut doc, &ics, now());
        assert_eq!((again.added, again.skipped), (0, 1));
        let id = doc.tasks().into_iter().find(|t| t.title == "Once").unwrap().id;
        doc.delete_task(&id);
        assert_eq!(import(&mut doc, &ics, now()).added, 0);
        assert!(doc.task(&id).is_none());

        // A second device importing the same file writes the same task id.
        let mut other = Doc::default();
        import(&mut other, &ics, now());
        assert!(other.task(&id).is_some());
    }

    #[test]
    fn past_events_are_history_and_past_repeats_resume() {
        let t = imported("SUMMARY:Old\r\nDTSTART:20200101T090000\r\n");
        assert!(t.done);
        let t = imported("SUMMARY:Weekly\r\nDTSTART:20261007T090000\r\nDTEND:20261007T100000\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\n");
        assert!(!t.done);
        assert_eq!(t.repeat, Repeat::Weekly);
        assert_eq!(t.start, parse_dt("2026-11-04T09:00").map(|d| d + Duration::days(7)));
        assert_eq!(t.end, parse_dt("2026-11-11T10:00"));
        // No longer repeats and already over: a dropped UNTIL rule mustn't look overdue.
        let t = imported("SUMMARY:Ended\r\nDTSTART:20200101T090000\r\nRRULE:FREQ=WEEKLY;UNTIL=20200201T000000Z\r\n");
        assert_eq!(t.repeat, Repeat::None);
        assert!(t.done);
    }

    #[test]
    fn rules_patch_cannot_express_are_dropped_and_reported() {
        let d = parse_dt("2026-12-01T09:00").unwrap(); // a Tuesday
        assert_eq!(repeat("FREQ=WEEKLY;BYDAY=TU", d), Some(Repeat::Weekly));
        assert_eq!(repeat("FREQ=WEEKLY;BYDAY=FR,MO,TU,WE,TH", d), Some(Repeat::Weekdays));
        assert_eq!(repeat("FREQ=WEEKLY;BYDAY=MO,WE", d), None);
        assert_eq!(repeat("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TU,WE,TH,FR", d), None);
        assert_eq!(repeat("FREQ=MONTHLY;BYDAY=2TU", d), None);
        assert_eq!(repeat("FREQ=DAILY;COUNT=5", d), None);
        assert_eq!(repeat("FREQ=DAILY;INTERVAL=0", d), None);
        assert_eq!(repeat("FREQ=HOURLY", d), None);
        assert_eq!(repeat("garbage", d), None);

        let mut doc = Doc::default();
        let report = import(&mut doc, &event("UID:r\r\nDTSTART:20261201T090000\r\nRRULE:FREQ=DAILY;COUNT=5\r\n"), now());
        assert_eq!((report.added, report.simplified), (1, 1));
        assert_eq!(doc.task("ics-r").unwrap().repeat, Repeat::None);
    }

    #[test]
    fn cancelled_overrides_and_undated_events_are_skipped() {
        let ics = calendar(concat!(
            "BEGIN:VEVENT\r\nUID:a\r\nSTATUS:CANCELLED\r\nDTSTART:20261201T090000\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:b\r\nRECURRENCE-ID:20261201T090000\r\nDTSTART:20261201T090000\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:No date\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:d\r\nDTSTART:not-a-date\r\nEND:VEVENT\r\n",
            "BEGIN:VTODO\r\nUID:e\r\nDTSTART:20261201T090000\r\nEND:VTODO\r\n",
        ));
        let mut doc = Doc::default();
        let report = import(&mut doc, &ics, now());
        assert_eq!((report.added, report.skipped), (0, 4));
    }

    #[test]
    fn untrusted_values_cannot_break_out_of_a_line_or_id() {
        let mut t = task("x\r\nEND:VEVENT\r\nBEGIN:VEVENT", "2026-12-01T09:00", None);
        t.id = "a\r\nX:y".into();
        let ics = export(&[t]);
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2, "one real, one escaped inside SUMMARY");
        assert_eq!(ics.lines().filter(|l| l.trim_end() == "BEGIN:VEVENT").count(), 1);

        let mut doc = Doc::default();
        import(&mut doc, &event("UID:../../etc/passwd\r\nDTSTART:20261201T090000\r\n"), now());
        assert!(doc.task("ics-------etc-passwd").is_some());
    }

    #[test]
    fn duplicate_uids_in_one_file_add_one_task() {
        let one = "BEGIN:VEVENT\r\nUID:dup\r\nDTSTART:20261201T090000\r\nEND:VEVENT\r\n";
        let mut doc = Doc::default();
        let report = import(&mut doc, &calendar(&one.repeat(2)), now());
        assert_eq!((report.added, report.skipped), (1, 1));
    }

    #[test]
    fn reimporting_only_reports_skips() {
        let ics = event("UID:z\r\nDTSTART;TZID=Asia/Tokyo:20261201T090000\r\nRRULE:FREQ=DAILY;COUNT=2\r\n");
        let mut doc = Doc::default();
        import(&mut doc, &ics, now());
        let again = import(&mut doc, &ics, now());
        assert_eq!(again, ImportReport { skipped: 1, ..ImportReport::default() });
    }

    #[test]
    fn imported_tasks_land_after_existing_cards_in_valid_columns() {
        let mut doc = Doc::default();
        let existing = doc.add_task(task("Existing", "2026-12-01T09:00", None));
        import(&mut doc, &calendar(concat!(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261201T090000\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261202T090000\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:old\r\nDTSTART:20200101T090000\r\nEND:VEVENT\r\n",
        )), now());
        let order = |id: &str| doc.task(id).unwrap().order;
        assert!(order(&existing) < order("ics-a") && order("ics-a") < order("ics-b"));
        let old = doc.task("ics-old").unwrap();
        assert!(old.done && old.done_at.is_some());
        assert_eq!(old.column, crate::doc::COL_DONE);
        assert_eq!(doc.task("ics-a").unwrap().column, COL_TODO);
        assert!(doc.has_pending(), "imported tasks sync");
        assert!(doc.task("ics-a").unwrap().created > 0);
    }

    #[test]
    fn big_calendars_import_quickly() {
        let body: String = (0..5000).map(|i| format!("BEGIN:VEVENT\r\nUID:{i}\r\nDTSTART:20261201T090000\r\nEND:VEVENT\r\n")).collect();
        let mut doc = Doc::default();
        let started = std::time::Instant::now();
        assert_eq!(import(&mut doc, &calendar(&body), now()).added, 5000);
        assert!(started.elapsed().as_secs() < 30, "rescanning per insert freezes the app");
    }

    #[test]
    fn calendar_detection() {
        assert!(is_calendar(&calendar("")));
        assert!(!is_calendar("hello"));
    }
}
