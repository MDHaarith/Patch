//! Typed views over records.

use crate::record::{Record, KIND_COLUMN, KIND_LIST, KIND_TASK};
use crate::time::{fmt_dt, parse_dt};
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    None,
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
}

impl Repeat {
    pub const ALL: [Repeat; 6] = [
        Repeat::None,
        Repeat::Daily,
        Repeat::Weekdays,
        Repeat::Weekly,
        Repeat::Monthly,
        Repeat::Yearly,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Repeat::None => "none",
            Repeat::Daily => "daily",
            Repeat::Weekdays => "weekdays",
            Repeat::Weekly => "weekly",
            Repeat::Monthly => "monthly",
            Repeat::Yearly => "yearly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::None => "Never",
            Repeat::Daily => "Every day",
            Repeat::Weekdays => "Weekdays",
            Repeat::Weekly => "Every week",
            Repeat::Monthly => "Every month",
            Repeat::Yearly => "Every year",
        }
    }

    pub fn parse(s: &str) -> Repeat {
        Repeat::ALL.into_iter().find(|r| r.as_str() == s).unwrap_or_default()
    }

    pub fn index(self) -> usize {
        Repeat::ALL.iter().position(|r| *r == self).unwrap_or(0)
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
    pub checklist: Vec<CheckItem>,
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
            checklist: Vec::new(),
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
            repeat: Repeat::parse(r.str("repeat").unwrap_or("none")),
            checklist: r
                .get("checklist")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            order: r.f64("order").unwrap_or(0.0),
            created: r.i64("created").unwrap_or(0),
        })
    }

    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
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
            ("repeat", json!(self.repeat.as_str())),
            ("checklist", serde_json::to_value(&self.checklist).unwrap_or(Value::Null)),
            ("order", json!(self.order)),
            ("created", json!(self.created)),
        ]
    }

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
