//! The local replica: all records plus sync bookkeeping and typed operations.

use crate::hlc::{now_ms, Clock, Stamp};
use crate::id::{new_id, random_string};
use crate::model::{Column, List, SmartView, Task};
use crate::record::{Record, KIND_COLUMN, KIND_LIST, KIND_TASK, KIND_VIEW};
use crate::recur;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

pub const COL_TODO: &str = "col-todo";
pub const COL_DOING: &str = "col-doing";
pub const COL_DONE: &str = "col-done";
pub const DEFAULT_LIST: &str = "list-personal";
pub const LIST_WORK: &str = "list-work";

#[derive(Serialize, Deserialize)]
struct DocFile {
    version: u32,
    node: String,
    clock: (u64, u32),
    last_seq: u64,
    hub_id: Option<String>,
    dirty: Vec<(String, Stamp)>,
    records: Vec<Record>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub today: u32,
    pub streak: u32,
    pub best: u32,
    /// Completions per day, oldest first, ending today.
    pub last_7: [u32; 7],
    pub focus_today: u32,
}

/// Values only: replay always goes through `write` with fresh clock stamps.
#[derive(Clone, Debug)]
pub struct ChangeSet(Vec<RecordChange>);

#[derive(Clone, Debug)]
struct RecordChange {
    id: String,
    kind: String,
    fields: Vec<(String, Value, Value)>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

#[derive(Clone, Debug)]
pub struct Doc {
    records: BTreeMap<String, Record>,
    clock: Clock,
    /// Records with unpushed local writes, with the stamp of the latest one.
    dirty: BTreeMap<String, Stamp>,
    /// Highest hub sequence number we have pulled.
    pub last_seq: u64,
    /// Identity of the hub `last_seq` refers to.
    pub hub_id: Option<String>,
    rev: u64,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::with_node(random_string(8))
    }
}

impl Doc {
    pub fn with_node(node: impl Into<String>) -> Doc {
        let mut doc = Doc {
            records: BTreeMap::new(),
            clock: Clock::new(node),
            dirty: BTreeMap::new(),
            last_seq: 0,
            hub_id: None,
            rev: 0,
        };
        doc.seed();
        doc
    }

    /// Built-in lists and columns. Every replica creates them with the same
    /// ids and the seed stamp, so they never conflict or duplicate.
    fn seed(&mut self) {
        type Seed<'a> = (&'a str, &'a str, Vec<(&'a str, Value)>);
        let seed = Stamp::seed();
        let defaults: [Seed; 5] = [
            (COL_TODO, KIND_COLUMN, vec![("name", json!("To Do")), ("done", json!(false)), ("order", json!(1.0))]),
            (COL_DOING, KIND_COLUMN, vec![("name", json!("In Progress")), ("done", json!(false)), ("order", json!(2.0))]),
            (COL_DONE, KIND_COLUMN, vec![("name", json!("Done")), ("done", json!(true)), ("order", json!(3.0))]),
            (DEFAULT_LIST, KIND_LIST, vec![("name", json!("Personal")), ("color", json!("#0A84FF")), ("order", json!(1.0))]),
            (LIST_WORK, KIND_LIST, vec![("name", json!("Work")), ("color", json!("#FF9F0A")), ("order", json!(2.0))]),
        ];
        for (id, kind, fields) in defaults {
            let rec = self.records.entry(id.to_string()).or_insert_with(|| Record::new(id, kind));
            for (k, v) in fields {
                rec.put(k, v, &seed);
            }
        }
    }

    pub fn node(&self) -> &str {
        &self.clock.node
    }

    /// Increments on every local or remote change; cheap change detection.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    // ---------------------------------------------------------------- storage

    pub fn to_json(&self) -> String {
        let file = DocFile {
            version: 1,
            node: self.clock.node.clone(),
            clock: self.clock.state(),
            last_seq: self.last_seq,
            hub_id: self.hub_id.clone(),
            dirty: self.dirty.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            records: self.records.values().cloned().collect(),
        };
        serde_json::to_string(&file).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<Doc> {
        let file: DocFile = serde_json::from_str(s).ok()?;
        let mut doc = Doc {
            records: file.records.into_iter().map(|r| (r.id.clone(), r)).collect(),
            clock: Clock::restore(file.node, file.clock.0, file.clock.1),
            dirty: file.dirty.into_iter().collect(),
            last_seq: file.last_seq,
            hub_id: file.hub_id,
            rev: 0,
        };
        doc.seed();
        Some(doc)
    }

    pub fn load_or_new(path: &Path) -> Doc {
        match std::fs::read(path) {
            Ok(bytes) => {
                if let Some(doc) = std::str::from_utf8(&bytes).ok().and_then(Doc::from_json) {
                    return doc;
                }
                preserve_corrupt(path).expect("failed to preserve corrupt document");
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => panic!("failed to read {}: {e}", path.display()),
        }
        Doc::default()
    }

    /// Atomic save: write a temp file and rename it over the old one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, self.to_json().as_bytes())
    }

    // ---------------------------------------------------------------- sync

    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }

    pub fn record(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    fn write(&mut self, id: &str, kind: &str, fields: Vec<(&str, Value)>) {
        if fields.is_empty() {
            return;
        }
        let stamp = self.clock.tick();
        let rec = self.records.entry(id.to_string()).or_insert_with(|| Record::new(id, kind));
        for (k, v) in fields {
            rec.put(k, v, &stamp);
        }
        self.dirty.insert(id.to_string(), stamp);
        self.rev += 1;
    }

    /// Merge records received from the hub. Returns true if anything changed.
    pub fn apply_remote(&mut self, incoming: &[Record]) -> bool {
        let mut changed = false;
        for rec in incoming {
            if let Some(s) = rec.max_stamp() {
                self.clock.observe(s);
            }
            match self.records.get_mut(&rec.id) {
                Some(local) => changed |= local.merge(rec),
                None => {
                    self.records.insert(rec.id.clone(), rec.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.rev += 1;
        }
        changed
    }

    /// Records with local changes the hub has not acknowledged yet.
    pub fn pending(&self) -> Vec<Record> {
        self.dirty.keys().filter_map(|id| self.records.get(id).cloned()).collect()
    }

    pub fn has_pending(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Clear dirty flags for records the hub accepted, unless they were
    /// edited again while the request was in flight.
    pub fn ack(&mut self, sent: &[Record]) {
        for rec in sent {
            let covered = match (self.dirty.get(&rec.id), rec.max_stamp()) {
                (Some(latest_local), Some(sent_max)) => sent_max >= latest_local,
                _ => false,
            };
            if covered {
                self.dirty.remove(&rec.id);
                self.rev += 1;
            }
        }
    }

    pub fn advance_seq(&mut self, seq: u64) {
        if seq > self.last_seq {
            self.last_seq = seq;
            self.rev += 1;
        }
    }

    /// Called with the hub id from each sync response. Switching to a
    /// different hub restarts pulling from scratch and re-pushes everything,
    /// so the new hub receives our full state.
    pub fn adopt_hub(&mut self, hub_id: &str) -> bool {
        if self.hub_id.as_deref() == Some(hub_id) {
            return false;
        }
        self.hub_id = Some(hub_id.to_string());
        self.rev += 1;
        self.last_seq = 0;
        let all: Vec<(String, Stamp)> = self
            .records
            .values()
            .filter_map(|r| r.max_stamp().filter(|s| **s != Stamp::seed()).map(|s| (r.id.clone(), s.clone())))
            .collect();
        self.dirty.extend(all);
        true
    }

    pub fn changes_since(&self, before: &Doc) -> ChangeSet {
        ChangeSet(self.records.values().filter_map(|record| {
            let old = before.record(&record.id);
            let mut fields: Vec<_> = record.f.iter().filter_map(|(key, field)| {
                let prev = old.and_then(|r| r.get(key)).cloned().unwrap_or(Value::Null);
                (prev != field.v).then(|| (key.clone(), prev, field.v.clone()))
            }).collect();
            if old.is_none() {
                fields = vec![("deleted".into(), json!(true), json!(false))];
            }
            (!fields.is_empty()).then(|| RecordChange { id: record.id.clone(), kind: record.kind.clone(), fields })
        }).collect())
    }

    pub fn replay(&mut self, changes: &ChangeSet, undo: bool) {
        for change in &changes.0 {
            self.write(&change.id, &change.kind, change.fields.iter().map(|(key, before, after)|
                (key.as_str(), if undo { before } else { after }.clone())).collect());
        }
    }

    pub fn smart_views(&self) -> Vec<SmartView> {
        self.records.values().filter_map(SmartView::from_record).collect()
    }

    pub fn save_smart_view_from(&mut self, base: &SmartView, view: &SmartView) {
        if base.id != view.id { return; }
        let before: BTreeMap<_, _> = base.fields().into_iter().collect();
        let mut after: BTreeMap<_, _> = view.fields().into_iter().collect();
        for key in before.keys().filter(|k| k.starts_with("tags.")) {
            after.entry(key.clone()).or_insert(Value::Null);
        }
        let fields = after.iter().filter(|(k,v)| before.get(*k) != Some(*v) || self.record(&view.id).is_none())
            .map(|(k,v)| (k.as_str(), v.clone())).collect();
        self.write(&view.id, KIND_VIEW, fields);
    }

    pub fn delete_smart_view(&mut self, id: &str) {
        self.write(id, KIND_VIEW, vec![("deleted", json!(true))]);
    }

    /// Validate local edits; remote cycles remain stored and traversals use visited sets.
    pub fn links_valid(&self, task: &Task) -> bool {
        for dependencies in [false, true] {
            let edges: Vec<_> = if dependencies { task.blockers.clone() } else { task.parent.iter().cloned().collect() };
            for edge in edges {
                let mut pending = vec![edge];
                let mut visited = std::collections::HashSet::new();
                while let Some(id) = pending.pop() {
                    if id == task.id { return false; }
                    if !visited.insert(id.clone()) { continue; }
                    if let Some(t) = self.task(&id) {
                        if dependencies { pending.extend(t.blockers); } else { pending.extend(t.parent); }
                    }
                }
            }
        }
        true
    }

    pub fn blocked(&self, task: &Task) -> bool {
        task.blockers.iter().any(|id| self.task(id).is_some_and(|t| !t.done && !t.archived))
    }

    pub fn duplicate_task(&mut self, id: &str) -> Option<String> {
        let mut t = self.task(id)?;
        t.id.clear();
        t.done = false;
        t.done_at = None;
        t.column = self.first_open_column();
        t.archived = false;
        t.template = false;
        if self.task(id)?.template {
            t.parent = None;
            t.start = None; t.end = None; t.repeat_anchor = None;
        }
        for item in &mut t.checklist { item.done = false; item.id = new_id("check"); }
        Some(self.add_task(t))
    }

    /// Rolls a repeating task to its next occurrence; false if it has none.
    fn advance_repeat(&mut self, id: &str, at: chrono::NaiveDateTime) -> bool {
        let Some(mut t) = self.task(id) else { return false };
        let base = t.clone();
        let Some(start) = t.start.filter(|_| t.repeat != crate::model::Repeat::None) else { return false };
        let Some(next) = recur::after_completion(t.repeat, at).or_else(|| recur::next_start(&t)) else { return false };
        t.repeat_anchor.get_or_insert(start);
        let next = if t.all_day { next.date().and_time(chrono::NaiveTime::MIN) } else { next };
        let delta = next - start;
        t.start = Some(next);
        t.end = t.end.map(|e| e + delta);
        for item in &mut t.checklist {
            item.done = false;
        }
        if self.column_is_done(&t.column) {
            t.column = self.first_open_column();
        }
        t.done = false;
        t.done_at = None;
        self.save_task_from(&base, &t);
        true
    }

    /// Skips one occurrence of a repeating task without counting it as done.
    pub fn skip_occurrence(&mut self, id: &str, now: chrono::NaiveDateTime) -> bool {
        self.advance_repeat(id, now)
    }

    /// Brings every overdue task up to date and returns how many moved. Calendar
    /// repeats roll past what they missed; anything else lands on today.
    pub fn reschedule_overdue(&mut self, now: chrono::NaiveDateTime) -> usize {
        use crate::model::Repeat;
        let overdue: Vec<Task> = self.tasks().into_iter().filter(|t| t.active() && t.is_overdue(now)).collect();
        for t in &overdue {
            if !matches!(t.repeat, Repeat::None | Repeat::AfterDone(..)) {
                // Bounded: a daily repeat that is years behind still catches up.
                for _ in 0..10_000 {
                    if !self.task(&t.id).is_some_and(|t| t.is_overdue(now)) || !self.advance_repeat(&t.id, now) { break; }
                }
                continue;
            }
            let Some(start) = t.start else { continue };
            let today = now.date().and_time(start.time());
            if t.all_day || today > now {
                self.reschedule(&t.id, today);
            } else {
                // Its time has passed today, so a timed slot would stay overdue.
                let mut moved = t.clone();
                moved.start = Some(now.date().and_time(chrono::NaiveTime::MIN));
                moved.end = None;
                moved.all_day = true;
                self.save_task(&moved);
            }
        }
        overdue.len()
    }

    /// Adds finished focus minutes to the task's total for that day.
    pub fn log_focus(&mut self, id: &str, minutes: i64, on: chrono::NaiveDate) {
        let key = format!("focus.{on}");
        let total = self.record(id).and_then(|r| r.i64(&key)).unwrap_or(0) + minutes;
        if self.task(id).is_some() {
            self.write(id, KIND_TASK, vec![(key.as_str(), json!(total))]);
        }
    }

    /// Reads deleted tasks too: removing a finished task shouldn't break a streak.
    pub fn stats(&self, today: chrono::NaiveDate) -> Stats {
        use chrono::{Duration, TimeZone};
        let (mut days, mut stats) = (BTreeMap::<chrono::NaiveDate, u32>::new(), Stats::default());
        let date_key = |key: &str, prefix: &str| key.strip_prefix(prefix).and_then(crate::time::parse_date);
        for r in self.records.values().filter(|r| r.kind == KIND_TASK) {
            if r.bool("done") {
                let done = r.i64("doneAt").and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single());
                if let Some(done) = done {
                    *days.entry(done.date_naive()).or_default() += 1;
                }
            }
            for (key, field) in &r.f {
                if field.v == true {
                    if let Some(day) = date_key(key, "completed.") {
                        *days.entry(day).or_default() += 1;
                    }
                } else if date_key(key, "focus.") == Some(today) {
                    stats.focus_today += field.v.as_i64().unwrap_or(0).max(0) as u32;
                }
            }
        }
        stats.today = days.get(&today).copied().unwrap_or(0);
        for (i, slot) in stats.last_7.iter_mut().enumerate() {
            *slot = days.get(&(today - Duration::days(6 - i as i64))).copied().unwrap_or(0);
        }
        // An empty today doesn't end the streak yet: there is still time left.
        let mut day = if stats.today > 0 { today } else { today - Duration::days(1) };
        while days.contains_key(&day) {
            stats.streak += 1;
            day -= Duration::days(1);
        }
        let mut run = (0, None);
        for &day in days.keys().filter(|d| **d <= today) {
            run.0 = if run.1 == Some(day - Duration::days(1)) { run.0 + 1 } else { 1 };
            run.1 = Some(day);
            stats.best = stats.best.max(run.0);
        }
        stats
    }

    pub fn auto_archive(&mut self, days: u32, now: i64) {
        // shortcut: restored tasks keep their completion age; track restoration time if restores should restart the policy.
        if days == 0 { return; }
        for mut task in self.tasks().into_iter().filter(|t| t.done && !t.archived && !t.template
            && t.done_at.is_some_and(|at| now.saturating_sub(at) >= i64::from(days) * 86_400_000)) {
            task.archived = true;
            self.save_task(&task);
        }
    }

    // ---------------------------------------------------------------- queries

    pub fn tasks(&self) -> Vec<Task> {
        let mut v: Vec<Task> = self.records.values().filter_map(Task::from_record).collect();
        v.sort_by(|a, b| a.order.total_cmp(&b.order).then(a.created.cmp(&b.created)));
        v
    }

    pub fn task(&self, id: &str) -> Option<Task> {
        self.records.get(id).and_then(Task::from_record)
    }

    pub fn lists(&self) -> Vec<List> {
        let mut v: Vec<List> = self.records.values().filter_map(List::from_record).collect();
        v.sort_by(|a, b| a.order.total_cmp(&b.order).then(a.name.cmp(&b.name)));
        v
    }

    pub fn list(&self, id: &str) -> Option<List> {
        self.records.get(id).and_then(List::from_record)
    }

    pub fn columns(&self) -> Vec<Column> {
        let mut v: Vec<Column> = self.records.values().filter_map(Column::from_record).collect();
        v.sort_by(|a, b| a.order.total_cmp(&b.order).then(a.name.cmp(&b.name)));
        v
    }

    pub fn column(&self, id: &str) -> Option<Column> {
        self.records.get(id).and_then(Column::from_record)
    }

    fn first_open_column(&self) -> String {
        self.columns()
            .into_iter()
            .find(|c| !c.done)
            .map(|c| c.id)
            .unwrap_or_else(|| COL_TODO.to_string())
    }

    fn first_done_column(&self) -> Option<String> {
        self.columns().into_iter().find(|c| c.done).map(|c| c.id)
    }

    fn column_is_done(&self, id: &str) -> bool {
        self.column(id).is_some_and(|c| c.done)
    }

    // ---------------------------------------------------------------- tasks

    /// Inserts a new task at the end of its column. Fills in id/created.
    pub fn add_task(&mut self, mut task: Task) -> String {
        if task.id.is_empty() {
            task.id = new_id("t");
        }
        if !self.links_valid(&task) { return String::new(); }
        if self.column(&task.column).is_none() {
            task.column = self.first_open_column();
        }
        if self.list(&task.list).is_none() {
            task.list = self.lists().first().map(|l| l.id.clone()).unwrap_or(DEFAULT_LIST.into());
        }
        task.done = self.column_is_done(&task.column);
        task.created = now_ms() as i64;
        task.order = self
            .tasks()
            .iter()
            .filter(|t| t.column == task.column)
            .map(|t| t.order)
            .fold(0.0, f64::max)
            + 1.0;
        let id = task.id.clone();
        self.write(&id, KIND_TASK, task.fields().iter().map(|(k, v)| (k.as_str(), v.clone())).collect());
        id
    }

    /// `add_task` for a whole calendar, whose per-insert rescan would take
    /// minutes. Tasks need ids; ones marked done go to the first done column.
    pub(crate) fn add_tasks(&mut self, tasks: Vec<Task>) {
        let columns = self.columns();
        let lists = self.lists();
        let open = self.first_open_column();
        let closed = self.first_done_column();
        let fallback_list = lists.first().map_or(DEFAULT_LIST.to_string(), |l| l.id.clone());
        let mut orders: BTreeMap<String, f64> = BTreeMap::new();
        for t in self.tasks() {
            let order = orders.entry(t.column).or_insert(0.0);
            *order = order.max(t.order);
        }
        let now = now_ms() as i64;
        for mut task in tasks {
            if task.done {
                task.done_at = Some(now);
                task.column = closed.clone().unwrap_or_else(|| open.clone());
            } else if !columns.iter().any(|c| c.id == task.column && !c.done) {
                task.column = open.clone();
            }
            if !lists.iter().any(|l| l.id == task.list) {
                task.list = fallback_list.clone();
            }
            task.created = now;
            let order = orders.entry(task.column.clone()).or_insert(0.0);
            *order += 1.0;
            task.order = *order;
            let id = task.id.clone();
            self.write(&id, KIND_TASK, task.fields().iter().map(|(k, v)| (k.as_str(), v.clone())).collect());
        }
    }

    /// Persists an immediate edit against the current task. Editor snapshots
    /// must use `save_task_from` with the task they started from.
    pub fn save_task(&mut self, task: &Task) {
        match self.task(&task.id) {
            Some(base) => {
                let mut task = task.clone();
                if task.start != base.start || task.repeat != base.repeat {
                    task.repeat_anchor = None;
                }
                self.save_task_from(&base, &task);
            }
            None => { self.add_task(task.clone()); }
        }
    }

    /// Writes only changes made since `base`, preserving concurrent edits.
    pub fn save_task_from(&mut self, base: &Task, task: &Task) {
        if base.id != task.id || self.task(&task.id).is_none() {
            return;
        }
        let mut task = task.clone();
        if (task.parent != base.parent || task.blockers != base.blockers) && !self.links_valid(&task) {
            return;
        }
        if task.column != base.column {
            task.done = self.column_is_done(&task.column);
        }
        if task.done && !base.done && task.repeat != crate::model::Repeat::None && task.start.is_some() {
            // Inspector status changes complete through the same recurrence path as a checkbox.
            task.done = false;
            task.done_at = None;
            task.column = base.column.clone();
            self.save_task_from(base, &task);
            self.set_done(&task.id, true);
            return;
        }
        if task.done && !base.done && task.done_at.is_none() {
            task.done_at = Some(now_ms() as i64);
        } else if !task.done && base.done {
            task.done_at = None;
        }
        let mut before: std::collections::BTreeMap<_, _> = base.fields().into_iter().collect();
        let mut after: std::collections::BTreeMap<_, _> = task.fields().into_iter().collect();
        let record = &self.records[&task.id];
        if record.f.keys().any(|key| key.starts_with("checklist.")) {
            let mut next_order = record.f.iter()
                .filter(|(key, _)| key.starts_with("checklist."))
                .filter_map(|(_, field)| field.v.get("order").and_then(Value::as_u64))
                .max().map_or(0, |order| order + 1);
            // Keep gaps on removal so unchanged items retain their values and stamps.
            for item in &task.checklist {
                let key = format!("checklist.{}", item.id);
                let order = record.get(&key).and_then(|value| value.get("order")).cloned()
                    .unwrap_or_else(|| {
                        let order = json!(next_order);
                        next_order += 1;
                        order
                    });
                if let Some(value) = before.get_mut(&key) {
                    value["order"] = order.clone();
                }
                after.get_mut(&key).unwrap()["order"] = order;
            }
        } else {
            before.retain(|key, _| !key.starts_with("checklist."));
        }
        for key in before.keys().filter(|key| ["checklist.", "tags.", "reminders.", "blockers."].iter().any(|prefix| key.starts_with(prefix))) {
            after.entry(key.clone()).or_insert(Value::Null);
        }
        let reset_anchor = task.repeat_anchor.is_none() && (task.start != base.start || task.repeat != base.repeat);
        let changed = after.iter().filter(|(key, value)| {
            (before.get(*key) != Some(*value) || (key.as_str() == "repeatAnchor" && reset_anchor)) && record.get(key) != Some(*value)
        })
            .map(|(key, value)| (key.as_str(), value.clone())).collect();
        self.write(&task.id, KIND_TASK, changed);
    }

    /// Completes or reopens a task. Completing a repeating task rolls it to
    /// its next occurrence instead.
    pub fn set_done(&mut self, id: &str, done: bool) {
        self.set_done_at(id, done, crate::time::now_local());
    }

    pub fn set_done_at(&mut self, id: &str, done: bool, completed: chrono::NaiveDateTime) {
        if done && self.advance_repeat(id, completed) {
            // The rolled-forward task keeps no done date, so streaks need this log.
            self.write(id, KIND_TASK, vec![(format!("completed.{}", completed.date()).as_str(), json!(true))]);
            return;
        }
        let Some(mut t) = self.task(id) else { return };
        t.done = done;
        if done {
            t.done_at = Some(now_ms() as i64);
            if !self.column_is_done(&t.column) {
                if let Some(col) = self.first_done_column() {
                    t.column = col;
                }
            }
        } else {
            t.done_at = None;
            if self.column_is_done(&t.column) {
                t.column = self.first_open_column();
            }
        }
        let fields = vec![
            ("done", json!(t.done)),
            ("doneAt", json!(t.done_at)),
            ("column", json!(t.column)),
        ];
        let current = self.task(id).map(|c| c.fields()).unwrap_or_default();
        let changed = fields.into_iter().filter(|(k, v)| !current.iter().any(|(ck, cv)| ck == k && cv == v)).collect();
        self.write(id, KIND_TASK, changed);
    }

    /// Moves a card to `column` at position `index` among that column's cards.
    pub fn move_task(&mut self, id: &str, column: &str, index: usize) {
        let Some(task) = self.task(id) else { return };
        if self.column(column).is_none() {
            return;
        }
        if self.column_is_done(column) && task.repeat != crate::model::Repeat::None && task.start.is_some() {
            self.set_done(id, true);
            return;
        }
        let siblings: Vec<f64> = self
            .tasks()
            .into_iter()
            .filter(|t| t.column == column && t.id != id)
            .map(|t| t.order)
            .collect();
        let index = index.min(siblings.len());
        let order = match (index.checked_sub(1).map(|i| siblings[i]), siblings.get(index)) {
            (None, None) => 1.0,
            (None, Some(next)) => next - 1.0,
            (Some(prev), None) => prev + 1.0,
            (Some(prev), Some(next)) => (prev + next) / 2.0,
        };
        let done = self.column_is_done(column);
        let mut fields = vec![("order", json!(order))];
        if task.column != column {
            fields.push(("column", json!(column)));
        }
        if task.done != done {
            fields.push(("done", json!(done)));
            fields.push(("doneAt", json!(if done { Some(now_ms() as i64) } else { None })));
        }
        self.write(id, KIND_TASK, fields);
    }

    /// Moves a task (keeping its duration) so that it starts at `start`.
    pub fn reschedule(&mut self, id: &str, start: chrono::NaiveDateTime) {
        let Some(mut t) = self.task(id) else { return };
        let delta = match t.start {
            Some(old) => start - old,
            None => {
                t.start = Some(start);
                self.save_task(&t);
                return;
            }
        };
        t.start = t.start.map(|s| s + delta);
        t.end = t.end.map(|e| e + delta);
        self.save_task(&t);
    }

    pub fn delete_task(&mut self, id: &str) {
        if self.task(id).is_some() {
            self.write(id, KIND_TASK, vec![("deleted", json!(true))]);
        }
    }

    pub fn restore_task(&mut self, id: &str) {
        if self.records.get(id).is_some_and(|r| r.kind == KIND_TASK && r.is_deleted()) {
            self.write(id, KIND_TASK, vec![("deleted", json!(false))]);
        }
    }

    // ---------------------------------------------------------------- lists

    pub fn add_list(&mut self, name: &str, color: &str) -> String {
        let id = new_id("l");
        let order = self.lists().iter().map(|l| l.order).fold(0.0, f64::max) + 1.0;
        let list = List { id: id.clone(), name: name.into(), color: color.into(), order };
        self.write(&id, KIND_LIST, list.fields());
        id
    }

    pub fn save_list(&mut self, list: &List) {
        let old = self.list(&list.id).map(|l| l.fields()).unwrap_or_default();
        let changed = list.fields().into_iter().filter(|f| !old.contains(f)).collect();
        self.write(&list.id.clone(), KIND_LIST, changed);
    }

    /// Deletes a list, moving its tasks to the first remaining list.
    pub fn delete_list(&mut self, id: &str) -> bool {
        let lists = self.lists();
        let Some(fallback) = lists.iter().find(|l| l.id != id).map(|l| l.id.clone()) else {
            return false;
        };
        for t in self.tasks().into_iter().filter(|t| t.list == id) {
            self.write(&t.id, KIND_TASK, vec![("list", json!(fallback))]);
        }
        self.write(id, KIND_LIST, vec![("deleted", json!(true))]);
        true
    }

    // ---------------------------------------------------------------- columns

    pub fn add_column(&mut self, name: &str) -> String {
        let id = new_id("c");
        let cols = self.columns();
        // New columns go before the first "done" column.
        let order = match cols.iter().position(|c| c.done) {
            Some(i) if i > 0 => (cols[i - 1].order + cols[i].order) / 2.0,
            Some(_) => cols[0].order - 1.0,
            None => cols.iter().map(|c| c.order).fold(0.0, f64::max) + 1.0,
        };
        let col = Column { id: id.clone(), name: name.into(), done: false, order };
        self.write(&id, KIND_COLUMN, col.fields());
        id
    }

    pub fn save_column(&mut self, col: &Column) {
        let old = self.column(&col.id).map(|c| c.fields()).unwrap_or_default();
        let changed = col.fields().into_iter().filter(|f| !old.contains(f)).collect();
        self.write(&col.id.clone(), KIND_COLUMN, changed);
    }

    pub fn move_column(&mut self, id: &str, index: usize) {
        let others: Vec<f64> = self.columns().into_iter().filter(|c| c.id != id).map(|c| c.order).collect();
        let index = index.min(others.len());
        let order = match (index.checked_sub(1).map(|i| others[i]), others.get(index)) {
            (None, None) => 1.0,
            (None, Some(n)) => n - 1.0,
            (Some(p), None) => p + 1.0,
            (Some(p), Some(n)) => (p + n) / 2.0,
        };
        self.write(id, KIND_COLUMN, vec![("order", json!(order))]);
    }

    /// Deletes a column, moving its cards to the first remaining open column.
    pub fn delete_column(&mut self, id: &str) -> bool {
        let cols = self.columns();
        let Some(fallback) = cols
            .iter()
            .filter(|c| c.id != id)
            .min_by_key(|c| c.done)
            .cloned()
        else {
            return false;
        };
        for t in self.tasks().into_iter().filter(|t| t.column == id) {
            self.write(
                &t.id,
                KIND_TASK,
                vec![("column", json!(fallback.id)), ("done", json!(fallback.done))],
            );
        }
        self.write(id, KIND_COLUMN, vec![("deleted", json!(true))]);
        true
    }
}

pub fn preserve_corrupt(path: &Path) -> io::Result<()> {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".corrupt-{}", now_ms()));
    let backup = Path::new(&name);
    std::fs::rename(path, backup)?;
    eprintln!("patch: preserved corrupt {} as {}", path.display(), backup.display());
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let _ = std::fs::File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repeat;
    use crate::time::parse_dt;

    #[test]
    fn corrupt_document_is_preserved() {
        let dir = std::env::temp_dir().join(format!("patch-corrupt-{}", random_string(10)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.json");
        std::fs::write(&path, b"garbage").unwrap();
        let doc = Doc::load_or_new(&path);
        assert_eq!(doc.columns().len(), 3);
        assert!(!doc.has_pending());
        assert!(!path.exists());
        let backup = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        assert!(backup.file_name().unwrap().to_str().unwrap().starts_with("doc.json.corrupt-"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"garbage");
        doc.save(&path).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), b"garbage");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn new_task(doc: &mut Doc, title: &str) -> String {
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.title = title.into();
        doc.add_task(t)
    }

    fn sync(a: &mut Doc, b: &mut Doc) {
        let pa = a.pending();
        let pb = b.pending();
        b.apply_remote(&pa);
        a.apply_remote(&pb);
        a.ack(&pa);
        b.ack(&pb);
    }

    #[test]
    fn checklist_items_merge_and_delete() {
        use crate::model::CheckItem;
        let mut a = Doc::with_node("a");
        let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
        task.checklist = vec![
            CheckItem { id: "one".into(), text: "One".into(), done: false },
            CheckItem { id: "two".into(), text: "Two".into(), done: false },
        ];
        let id = a.add_task(task);
        let mut b = Doc::with_node("b");
        sync(&mut a, &mut b);
        let mut task = a.task(&id).unwrap();
        task.checklist[0].done = true;
        a.save_task(&task);
        let mut task = b.task(&id).unwrap();
        task.checklist[1].text = "Edited".into();
        b.save_task(&task);
        sync(&mut a, &mut b);
        let mut task = a.task(&id).unwrap();
        assert_eq!(Some(task.clone()), b.task(&id));
        assert!(task.checklist[0].done);
        assert_eq!(task.checklist[1].text, "Edited");
        task.checklist.remove(0);
        a.save_task(&task);
        sync(&mut a, &mut b);
        assert_eq!(Some(task), b.task(&id));
        assert_eq!(b.record(&id).unwrap().get("checklist.one"), Some(&Value::Null));
    }

    #[test]
    fn checklist_removal_preserves_concurrent_ticks_and_append_order() {
        use crate::model::CheckItem;
        for removal_is_newer in [false, true] {
            let mut a = Doc::with_node("a");
            let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
            task.checklist = ["one", "two", "three"].into_iter().map(|id| CheckItem {
                id: id.into(), text: id.into(), done: false,
            }).collect();
            let id = a.add_task(task);
            let mut b = Doc::with_node("b");
            sync(&mut a, &mut b);
            let before = a.record(&id).unwrap().clone();
            let mut removed = a.task(&id).unwrap();
            removed.checklist.remove(0);
            let mut ticked = b.task(&id).unwrap();
            ticked.checklist[2].done = true;
            if removal_is_newer {
                b.save_task(&ticked);
                std::thread::sleep(std::time::Duration::from_millis(2));
                a.save_task(&removed);
            } else {
                a.save_task(&removed);
                std::thread::sleep(std::time::Duration::from_millis(2));
                b.save_task(&ticked);
            }
            let removal_stamp = &a.record(&id).unwrap().f["checklist.one"].t;
            let tick_stamp = &b.record(&id).unwrap().f["checklist.three"].t;
            assert_eq!(removal_stamp > tick_stamp, removal_is_newer);
            assert_ne!(removal_stamp.parts().unwrap().0, tick_stamp.parts().unwrap().0);
            for key in ["checklist.two", "checklist.three"] {
                assert_eq!(a.record(&id).unwrap().f[key], before.f[key]);
            }
            sync(&mut a, &mut b);
            removed.checklist[1].done = true;
            assert_eq!(a.task(&id), Some(removed.clone()));
            assert_eq!(b.task(&id), Some(removed.clone()));

            // IDs deliberately sort before the survivors; stored order must put new items last.
            for id in ["aa", "a"] {
                removed.checklist.push(CheckItem { id: id.into(), text: id.into(), done: false });
            }
            a.save_task(&removed);
            sync(&mut a, &mut b);
            assert_eq!(a.task(&id), Some(removed.clone()));
            assert_eq!(b.task(&id), Some(removed));
        }
    }

    #[test]
    fn legacy_checklist_loads() {
        let mut rec = Record::new("legacy", KIND_TASK);
        rec.put("checklist", json!([{"id": "one", "text": "Old", "done": true}]), &Stamp::seed());
        let task = Task::from_record(&rec).unwrap();
        assert_eq!(task.checklist[0].text, "Old");
        assert!(task.checklist[0].done);
        let mut d = Doc::with_node("migration");
        d.apply_remote(&[rec]);
        let mut task = d.task("legacy").unwrap();
        task.notes = "Migrate".into();
        d.save_task(&task);
        assert_eq!(d.task("legacy"), Some(task));
        assert!(d.record("legacy").unwrap().get("checklist.one").is_some());
    }

    #[test]
    fn sync_progress_changes_revision_only_when_needed() {
        let mut d = Doc::default();
        new_task(&mut d, "Pending");
        let sent = d.pending();
        let rev = d.rev();
        d.ack(&sent);
        assert!(d.rev() > rev);
        let rev = d.rev();
        d.ack(&sent);
        d.advance_seq(0);
        assert_eq!(d.rev(), rev);
        d.advance_seq(10);
        assert!(d.rev() > rev);
        let loaded = Doc::from_json(&d.to_json()).unwrap();
        assert!(!loaded.has_pending());
        assert_eq!(loaded.last_seq, 10);
    }

    #[test]
    fn seeds_are_stable_and_clean() {
        let a = Doc::default();
        let b = Doc::default();
        assert_eq!(a.columns().len(), 3);
        assert_eq!(a.lists().len(), 2);
        assert!(!a.has_pending());
        assert_eq!(a.record(COL_TODO), b.record(COL_TODO));
    }

    #[test]
    fn concurrent_edits_to_different_fields_merge() {
        let mut a = Doc::with_node("a");
        let id = new_task(&mut a, "Buy milk");
        let mut b = Doc::with_node("b");
        sync(&mut a, &mut b);
        assert_eq!(b.task(&id).unwrap().title, "Buy milk");

        let mut ta = a.task(&id).unwrap();
        ta.title = "Buy oat milk".into();
        a.save_task(&ta);
        let mut tb = b.task(&id).unwrap();
        tb.notes = "2 litres".into();
        b.save_task(&tb);

        sync(&mut a, &mut b);
        let (ta, tb) = (a.task(&id).unwrap(), b.task(&id).unwrap());
        assert_eq!(ta, tb);
        assert_eq!(ta.title, "Buy oat milk");
        assert_eq!(ta.notes, "2 litres");
        assert!(!a.has_pending() && !b.has_pending());
    }

    #[test]
    fn done_moves_between_columns() {
        let mut d = Doc::with_node("a");
        let id = new_task(&mut d, "Ship it");
        d.set_done(&id, true);
        let t = d.task(&id).unwrap();
        assert!(t.done);
        assert_eq!(t.column, COL_DONE);
        d.set_done(&id, false);
        assert_eq!(d.task(&id).unwrap().column, COL_TODO);

        d.move_task(&id, COL_DONE, 0);
        assert!(d.task(&id).unwrap().done);
        d.move_task(&id, COL_DOING, 0);
        let t = d.task(&id).unwrap();
        assert!(!t.done);
        assert_eq!(t.column, COL_DOING);
    }

    #[test]
    fn ordering_inside_columns() {
        let mut d = Doc::with_node("a");
        let a = new_task(&mut d, "a");
        let b = new_task(&mut d, "b");
        let c = new_task(&mut d, "c");
        d.move_task(&c, COL_TODO, 0);
        let order: Vec<String> = d.tasks().into_iter().map(|t| t.title).collect();
        assert_eq!(order, vec!["c", "a", "b"]);
        d.move_task(&a, COL_TODO, 2);
        let order: Vec<String> = d.tasks().into_iter().map(|t| t.title).collect();
        assert_eq!(order, vec!["c", "b", "a"]);
        let _ = b;
    }

    #[test]
    fn stale_editor_saves_only_user_changes() {
        let mut a = Doc::with_node("a");
        let id = new_task(&mut a, "Original");
        let base = a.task(&id).unwrap();
        let mut draft = base.clone();
        draft.title = "Edited".into();
        let mut b = Doc::with_node("b");
        sync(&mut a, &mut b);
        let mut remote = b.task(&id).unwrap();
        remote.notes = "Remote notes".into();
        b.save_task(&remote);
        b.set_done(&id, true);
        sync(&mut a, &mut b);
        a.save_task_from(&base, &draft);
        let saved = a.task(&id).unwrap();
        assert_eq!(saved.title, "Edited");
        assert_eq!(saved.notes, "Remote notes");
        assert!(saved.done);
        assert_eq!(saved.column, COL_DONE);
        let rev = a.rev();
        a.save_task_from(&base, &base);
        assert_eq!(a.rev(), rev);
        a.delete_task(&id);
        a.save_task_from(&base, &draft);
        assert!(a.task(&id).is_none());
    }

    #[test]
    fn stale_checklist_keeps_remote_changes() {
        use crate::model::CheckItem;
        let mut d = Doc::with_node("a");
        let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
        task.checklist = vec![CheckItem { id: "one".into(), text: "One".into(), done: false }];
        let id = d.add_task(task);
        let base = d.task(&id).unwrap();
        let mut remote = base.clone();
        remote.checklist[0].done = true;
        remote.checklist.push(CheckItem { id: "two".into(), text: "Two".into(), done: false });
        d.save_task(&remote);
        let mut draft = base.clone();
        draft.notes = "Draft".into();
        d.save_task_from(&base, &draft);
        assert_eq!(d.task(&id).unwrap().checklist, remote.checklist);
    }

    #[test]
    fn dragging_repeat_to_done_rolls_forward() {
        use crate::model::CheckItem;
        let mut d = Doc::with_node("a");
        let mut task = Task::blank("", DEFAULT_LIST, COL_DOING);
        task.start = parse_dt("2026-10-13T10:00");
        task.repeat = Repeat::Daily;
        task.checklist.push(CheckItem { id: "one".into(), text: "One".into(), done: true });
        let id = d.add_task(task);
        d.move_task(&id, COL_DONE, 0);
        let task = d.task(&id).unwrap();
        assert_eq!(task.start, parse_dt("2026-10-14T10:00"));
        assert_eq!(task.column, COL_DOING);
        assert!(!task.done);
        assert!(!task.checklist[0].done);
    }

    #[test]
    fn completion_preserves_calendar_anchor() {
        for (repeat, start, dates) in [
            (Repeat::Monthly, "2026-01-31T10:00", vec!["2026-02-28T10:00", "2026-03-31T10:00", "2026-04-30T10:00"]),
            (Repeat::Yearly, "2024-02-29T10:00", vec!["2025-02-28T10:00", "2026-02-28T10:00", "2027-02-28T10:00", "2028-02-29T10:00"]),
            (Repeat::Every(2, crate::model::Unit::Month), "2026-12-31T10:00", vec!["2027-02-28T10:00", "2027-04-30T10:00", "2027-06-30T10:00", "2027-08-31T10:00"]),
        ] {
            let mut d = Doc::with_node("a");
            let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
            task.start = parse_dt(start);
            task.repeat = repeat;
            let id = d.add_task(task);
            for date in dates {
                d.set_done(&id, true);
                assert_eq!(d.task(&id).unwrap().start, parse_dt(date));
                d = Doc::from_json(&d.to_json()).unwrap();
            }
            d.reschedule(&id, parse_dt("2030-01-15T10:00").unwrap());
            assert_eq!(d.task(&id).unwrap().repeat_anchor, None);
        }
    }

    #[test]
    fn stale_reschedule_clears_remote_calendar_anchor() {
        let mut d = Doc::with_node("a");
        let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
        task.start = parse_dt("2026-01-31T10:00");
        task.repeat = Repeat::Monthly;
        let id = d.add_task(task);
        let base = d.task(&id).unwrap();
        d.set_done(&id, true);
        let mut draft = base.clone();
        draft.start = parse_dt("2026-03-15T10:00");
        d.save_task_from(&base, &draft);
        assert_eq!(d.task(&id).unwrap().repeat_anchor, None);
        d.set_done(&id, true);
        assert_eq!(d.task(&id).unwrap().start, parse_dt("2026-04-15T10:00"));
    }

    #[test]
    fn repeating_task_rolls_forward() {
        let mut d = Doc::with_node("a");
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.title = "Standup".into();
        t.start = parse_dt("2026-10-13T10:00");
        t.end = parse_dt("2026-10-13T10:15");
        t.repeat = Repeat::Daily;
        let id = d.add_task(t);
        d.set_done(&id, true);
        let t = d.task(&id).unwrap();
        assert!(!t.done);
        assert_eq!(t.start, parse_dt("2026-10-14T10:00"));
        assert_eq!(t.end, parse_dt("2026-10-14T10:15"));
    }

    #[test]
    fn restored_task_syncs() {
        let mut a = Doc::with_node("a");
        let mut b = Doc::with_node("b");
        let id = new_task(&mut a, "restore me");
        sync(&mut a, &mut b);
        a.delete_task(&id);
        sync(&mut a, &mut b);
        assert!(a.task(&id).is_none() && b.task(&id).is_none());
        a.restore_task(&id);
        sync(&mut a, &mut b);
        assert!(a.task(&id).is_some() && b.task(&id).is_some());
    }

    #[test]
    fn deletes_and_hub_switch() {
        let mut d = Doc::with_node("a");
        let id = new_task(&mut d, "temp");
        d.ack(&d.pending());
        d.delete_task(&id);
        assert!(d.task(&id).is_none());
        assert!(d.has_pending());
        d.ack(&d.pending());
        d.last_seq = 10;
        assert!(d.adopt_hub("hub-1"));
        assert_eq!(d.last_seq, 0);
        assert_eq!(d.pending().len(), 1);
        assert!(!d.adopt_hub("hub-1"));
    }

    #[test]
    fn json_round_trip() {
        let mut d = Doc::with_node("a");
        new_task(&mut d, "persist me");
        let s = d.to_json();
        let back = Doc::from_json(&s).unwrap();
        assert_eq!(back.tasks(), d.tasks());
        assert_eq!(back.node(), "a");
        assert!(back.has_pending());
    }

    #[test]
    fn delete_column_moves_cards() {
        let mut d = Doc::with_node("a");
        let col = d.add_column("Review");
        let cols: Vec<String> = d.columns().into_iter().map(|c| c.name).collect();
        assert_eq!(cols, vec!["To Do", "In Progress", "Review", "Done"]);
        let id = new_task(&mut d, "x");
        d.move_task(&id, &col, 0);
        assert!(d.delete_column(&col));
        assert_eq!(d.task(&id).unwrap().column, COL_TODO);
    }
}
