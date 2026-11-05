//! Typed views over records.

use crate::record::{Record, KIND_COLUMN, KIND_LIST, KIND_TASK, KIND_VIEW};
use crate::time::{fmt_dt, parse_dt};
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Day, Unit::Week, Unit::Month, Unit::Year];

    fn as_str(self) -> &'static str {
        match self {
            Unit::Day => "day",
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
        }
    }

    fn parse(s: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.as_str() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Repeat {
    #[default]
    None,
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
    /// Every `n` units, e.g. every 2 weeks.
    Every(u16, Unit),
    AfterDone(u16, Unit),
}

impl Repeat {
    /// The built-in choices, in menu order.
    pub const ALL: [Repeat; 7] = [
        Repeat::None,
        Repeat::Daily,
        Repeat::Weekdays,
        Repeat::Weekly,
        Repeat::Monthly,
        Repeat::Yearly,
        Repeat::AfterDone(1, Unit::Day),
    ];

    /// Stored form: "weekly", or "every:2:week" for custom intervals.
    pub fn encode(self) -> String {
        match self {
            Repeat::None => "none".into(),
            Repeat::Daily => "daily".into(),
            Repeat::Weekdays => "weekdays".into(),
            Repeat::Weekly => "weekly".into(),
            Repeat::Monthly => "monthly".into(),
            Repeat::Yearly => "yearly".into(),
            Repeat::Every(n, u) => format!("every:{n}:{}", u.as_str()),
            Repeat::AfterDone(n, u) => format!("after:{n}:{}", u.as_str()),
        }
    }

    pub fn label(self) -> String {
        match self {
            Repeat::None => "Never".into(),
            Repeat::Daily => "Every day".into(),
            Repeat::Weekdays => "Weekdays".into(),
            Repeat::Weekly => "Every week".into(),
            Repeat::Monthly => "Every month".into(),
            Repeat::Yearly => "Every year".into(),
            Repeat::AfterDone(n, u) => format!("{} after finishing", if n == 1 { format!("1 {}", u.as_str()) } else { format!("{n} {}s", u.as_str()) }),
            Repeat::Every(1, u) => Repeat::Every(1, u).normalized().label(),
            Repeat::Every(n, u) => format!("Every {n} {}s", u.as_str()),
        }
    }

    pub fn parse(s: &str) -> Repeat {
        if let Some(rest) = s.strip_prefix("every:").or_else(|| s.strip_prefix("after:")) {
            let mut it = rest.split(':');
            let n = it.next().and_then(|n| n.parse::<u16>().ok()).filter(|n| *n > 0);
            let u = it.next().and_then(Unit::parse);
            return match (n, u) {
                (Some(n), Some(u)) if s.starts_with("after:") => Repeat::AfterDone(n, u),
                (Some(n), Some(u)) => Repeat::Every(n, u).normalized(),
                _ => Repeat::None,
            };
        }
        Repeat::ALL.into_iter().find(|r| r.encode() == s).unwrap_or_default()
    }

    /// "Every 1 week" is just weekly.
    pub fn normalized(self) -> Repeat {
        match self {
            Repeat::Every(1, Unit::Day) => Repeat::Daily,
            Repeat::Every(1, Unit::Week) => Repeat::Weekly,
            Repeat::Every(1, Unit::Month) => Repeat::Monthly,
            Repeat::Every(1, Unit::Year) => Repeat::Yearly,
            r => r,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub list: String,
    pub column: String,
    pub done: bool,
    pub done_at: Option<i64>,
    /// 0 none, 1 low, 2 medium, 3 high.
    pub priority: u8,
    pub start: Option<NaiveDateTime>,
    /// Exclusive end for timed tasks; the last day (inclusive) for all-day tasks.
    pub end: Option<NaiveDateTime>,
    pub all_day: bool,
    /// Minutes before `start` to remind, if any.
    pub reminder: Option<i64>,
    pub repeat: Repeat,
    /// Original calendar anchor, retained when completion advances a repeat.
    pub repeat_anchor: Option<NaiveDateTime>,
    pub checklist: Vec<CheckItem>,
    pub tags: Vec<String>,
    /// Additional offsets; the legacy reminder remains independently readable.
    pub reminders: Vec<i64>,
    pub template: bool,
    pub archived: bool,
    pub parent: Option<String>,
    pub blockers: Vec<String>,
    pub order: f64,
    pub created: i64,
}

fn opt_dt(v: Option<NaiveDateTime>) -> Value {
    v.map(|d| Value::String(fmt_dt(d))).unwrap_or(Value::Null)
}

impl Task {
    pub fn blank(id: impl Into<String>, list: &str, column: &str) -> Task {
        Task {
            id: id.into(),
            title: String::new(),
            notes: String::new(),
            list: list.into(),
            column: column.into(),
            done: false,
            done_at: None,
            priority: 0,
            start: None,
            end: None,
            all_day: false,
            reminder: None,
            repeat: Repeat::None,
            repeat_anchor: None,
            checklist: Vec::new(),
            tags: Vec::new(),
            reminders: Vec::new(),
            template: false,
            archived: false,
            parent: None,
            blockers: Vec::new(),
            order: 0.0,
            created: 0,
        }
    }

    pub fn from_record(r: &Record) -> Option<Task> {
        if r.kind != KIND_TASK || r.is_deleted() {
            return None;
        }
        Some(Task {
            id: r.id.clone(),
            title: r.str("title").unwrap_or_default().to_string(),
            notes: r.str("notes").unwrap_or_default().to_string(),
            list: r.str("list").unwrap_or(crate::doc::DEFAULT_LIST).to_string(),
            column: r.str("column").unwrap_or(crate::doc::COL_TODO).to_string(),
            done: r.bool("done"),
            done_at: r.i64("doneAt"),
            priority: r.i64("priority").unwrap_or(0).clamp(0, 3) as u8,
            start: r.str("start").and_then(parse_dt),
            end: r.str("end").and_then(parse_dt),
            all_day: r.bool("allDay"),
            reminder: r.i64("reminder"),
            reminders: r.f.iter().filter_map(|(key, field)| key.strip_prefix("reminders.").filter(|_| field.v == true)?.parse::<i64>().ok()).filter(|m| (0..=525600).contains(m)).collect(),
            tags: members(r, "tags."),
            blockers: members(r, "blockers."),
            parent: r.str("parent").filter(|s| !s.is_empty()).map(str::to_string),
            template: r.bool("template"),
            archived: r.bool("archived"),
            repeat: Repeat::parse(r.str("repeat").unwrap_or("none")),
            repeat_anchor: r.str("repeatAnchor").and_then(parse_dt),
            checklist: if r.f.keys().any(|key| key.starts_with("checklist.")) {
                let mut items: Vec<_> = r.f.iter().filter_map(|(key, field)| {
                    let id = key.strip_prefix("checklist.")?;
                    Some((field.v.get("order").and_then(Value::as_u64).unwrap_or(0), CheckItem {
                        id: id.to_string(),
                        text: field.v.get("text")?.as_str()?.to_string(),
                        done: field.v.get("done").and_then(Value::as_bool).unwrap_or(false),
                    }))
                }).collect();
                items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
                items.into_iter().map(|(_, item)| item).collect()
            } else {
                r.get("checklist").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
            },
            order: r.f64("order").unwrap_or(0.0),
            created: r.i64("created").unwrap_or(0),
        })
    }

    pub fn fields(&self) -> Vec<(String, Value)> {
        let mut fields: Vec<(String, Value)> = vec![
            ("title", json!(self.title)),
            ("notes", json!(self.notes)),
            ("list", json!(self.list)),
            ("column", json!(self.column)),
            ("done", json!(self.done)),
            ("doneAt", json!(self.done_at)),
            ("priority", json!(self.priority)),
            ("start", opt_dt(self.start)),
            ("end", opt_dt(self.end)),
            ("allDay", json!(self.all_day)),
            ("reminder", json!(self.reminder)),
            ("repeat", json!(self.repeat.encode())),
            ("repeatAnchor", opt_dt(self.repeat_anchor)),
            ("checklist", serde_json::to_value(&self.checklist).unwrap_or(Value::Null)),
            ("order", json!(self.order)),
            ("created", json!(self.created)),
            ("template", json!(self.template)),
            ("archived", json!(self.archived)),
            ("parent", json!(self.parent)),
        ].into_iter().map(|(key, value)| (key.to_string(), value)).collect();
        fields.extend(self.checklist.iter().enumerate().map(|(order, item)| (
            format!("checklist.{}", item.id), json!({"text": item.text, "done": item.done, "order": order})
        )));
        fields.extend(self.tags.iter().map(|tag| (format!("tags.{tag}"), json!(true))));
        fields.extend(self.reminders.iter().map(|m| (format!("reminders.{m}"), json!(true))));
        fields.extend(self.blockers.iter().map(|id| (format!("blockers.{id}"), json!(true))));
        fields
    }

    pub fn reminder_offsets(&self) -> Vec<i64> {
        let mut offsets = self.reminders.clone();
        offsets.extend(self.reminder);
        offsets.sort_unstable();
        offsets.dedup();
        offsets
    }

    pub fn active(&self) -> bool { !self.template && !self.archived }

    /// Exclusive end of the first occurrence.
    pub fn span_end(&self) -> Option<NaiveDateTime> {
        let start = self.start?;
        Some(if self.all_day {
            let last = self.end.unwrap_or(start).max(start);
            last.date().and_time(chrono::NaiveTime::MIN) + Duration::days(1)
        } else {
            match self.end {
                Some(end) if end > start => end,
                _ => start + Duration::hours(1),
            }
        })
    }

    pub fn duration(&self) -> Duration {
        match (self.start, self.span_end()) {
            (Some(s), Some(e)) => e - s,
            _ => Duration::hours(1),
        }
    }

    pub fn is_overdue(&self, now: NaiveDateTime) -> bool {
        !self.done && self.span_end().is_some_and(|end| end <= now)
    }

    pub fn checklist_progress(&self) -> (usize, usize) {
        (self.checklist.iter().filter(|c| c.done).count(), self.checklist.len())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub id: String,
    pub name: String,
    pub color: String,
    pub order: f64,
}

impl List {
    pub fn from_record(r: &Record) -> Option<List> {
        if r.kind != KIND_LIST || r.is_deleted() {
            return None;
        }
        Some(List {
            id: r.id.clone(),
            name: r.str("name").unwrap_or("Untitled").to_string(),
            color: r.str("color").unwrap_or("#0A84FF").to_string(),
            order: r.f64("order").unwrap_or(0.0),
        })
    }

    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("name", json!(self.name)),
            ("color", json!(self.color)),
            ("order", json!(self.order)),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub id: String,
    pub name: String,
    /// Cards dropped here count as completed.
    pub done: bool,
    pub order: f64,
}

impl Column {
    pub fn from_record(r: &Record) -> Option<Column> {
        if r.kind != KIND_COLUMN || r.is_deleted() {
            return None;
        }
        Some(Column {
            id: r.id.clone(),
            name: r.str("name").unwrap_or("Untitled").to_string(),
            done: r.bool("done"),
            order: r.f64("order").unwrap_or(0.0),
        })
    }

    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("name", json!(self.name)),
            ("done", json!(self.done)),
            ("order", json!(self.order)),
        ]
    }
}

/// macOS Calendar's palette, in the order the color picker shows it.
pub const PALETTE: [(&str, &str); 10] = [
    ("Red", "#FF453A"),
    ("Orange", "#FF9F0A"),
    ("Yellow", "#FFD60A"),
    ("Green", "#30D158"),
    ("Teal", "#40C8E0"),
    ("Blue", "#0A84FF"),
    ("Indigo", "#5E5CE6"),
    ("Purple", "#BF5AF2"),
    ("Pink", "#FF375F"),
    ("Brown", "#AC8E68"),
];

/// Web links in notes. Notes sync from other devices, so only http(s) passes.
pub fn links_in(text: &str) -> Vec<String> {
    let mut links: Vec<String> = text.split_whitespace()
        .map(|word| word.trim_start_matches(['(', '[', '<', '"', '\'']).trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']))
        .filter(|word| ["http://", "https://"].iter().any(|scheme| word.len() > scheme.len() && word.starts_with(scheme)))
        .map(str::to_string)
        .collect();
    links.dedup();
    links
}

fn members(r: &Record, prefix: &str) -> Vec<String> {
    r.f.iter().filter_map(|(key, field)| {
        key.strip_prefix(prefix).filter(|_| field.v == true).map(str::to_string)
    }).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub tags: Vec<String>,
    pub list: Option<String>,
    pub priority: Option<u8>,
    /// any, overdue, today, week.
    pub due: String,
    pub include_done: bool,
}

impl Filter {
    pub fn matches(&self, t: &Task, now: NaiveDateTime) -> bool {
        (self.include_done || !t.done)
            && self.tags.iter().all(|tag| t.tags.contains(tag))
            && self.list.as_ref().is_none_or(|list| t.list == *list)
            && self.priority.is_none_or(|p| p == t.priority)
            && match self.due.as_str() {
                "overdue" => t.is_overdue(now),
                "today" => !crate::recur::occurrences(t, now.date().and_time(chrono::NaiveTime::MIN), (now.date() + Duration::days(1)).and_time(chrono::NaiveTime::MIN)).is_empty(),
                "week" => !crate::recur::occurrences(t, crate::time::week_start(now.date(), true).and_time(chrono::NaiveTime::MIN), (crate::time::week_start(now.date(), true) + Duration::days(7)).and_time(chrono::NaiveTime::MIN)).is_empty(),
                _ => true,
            }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmartView {
    pub id: String,
    pub name: String,
    pub filter: Filter,
}

impl SmartView {
    pub fn from_record(r: &Record) -> Option<Self> {
        (r.kind == KIND_VIEW && !r.is_deleted()).then(|| Self {
            id: r.id.clone(), name: r.str("name").unwrap_or("Smart view").into(),
            filter: Filter {
                tags: members(r, "tags."), list: r.str("list").map(str::to_string),
                priority: r.i64("priority").filter(|p| (0..=3).contains(p)).map(|p| p as u8),
                due: r.str("due").unwrap_or("any").into(), include_done: r.bool("includeDone"),
            },
        })
    }

    pub fn fields(&self) -> Vec<(String, Value)> {
        let mut fields: Vec<_> = vec![("name", json!(self.name)), ("list", json!(self.filter.list)),
            ("priority", json!(self.filter.priority)), ("due", json!(self.filter.due)),
            ("includeDone", json!(self.filter.include_done))].into_iter().map(|(k,v)| (k.into(),v)).collect();
        fields.extend(self.filter.tags.iter().map(|tag| (format!("tags.{tag}"), json!(true))));
        fields
    }
}
