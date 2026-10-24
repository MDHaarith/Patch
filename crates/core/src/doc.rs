//! The local replica: all records plus sync bookkeeping and typed operations.

use crate::hlc::{now_ms, Clock, Stamp};
use crate::id::{new_id, random_string};
use crate::model::{Column, List, Task};
use crate::record::{Record, KIND_COLUMN, KIND_LIST, KIND_TASK};
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

    /// Persists an edited task, writing only the fields that changed so
    /// concurrent edits to other fields on other devices survive.
    pub fn save_task(&mut self, task: &Task) {
        let mut task = task.clone();
        match self.task(&task.id) {
            None => {
                self.add_task(task);
            }
            Some(old) => {
                if task.column != old.column {
                    task.done = self.column_is_done(&task.column);
                }
                if task.done && !old.done && task.done_at.is_none() {
                    task.done_at = Some(now_ms() as i64);
                }
                let before: std::collections::BTreeMap<_, _> = old.fields().into_iter().collect();
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
                        after.get_mut(&key).unwrap()["order"] = order;
                    }
                }
                for key in before.keys().filter(|key| key.starts_with("checklist.")) {
                    after.entry(key.clone()).or_insert(Value::Null);
                }
                let changed = after.iter().filter(|(key, value)| self.records[&task.id].get(key) != Some(*value))
                    .map(|(key, value)| (key.as_str(), value.clone())).collect();
                self.write(&task.id, KIND_TASK, changed);
            }
        }
    }

    /// Completes or reopens a task. Completing a repeating task rolls it to
    /// its next occurrence instead.
    pub fn set_done(&mut self, id: &str, done: bool) {
        let Some(mut t) = self.task(id) else { return };
        if done && t.repeat != crate::model::Repeat::None && t.start.is_some() {
            if let (Some(start), Some(next)) = (t.start, recur::next_start(&t)) {
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
                self.save_task(&t);
                return;
            }
        }
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
    let mut file = std::fs::File::create(&tmp)?;
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
