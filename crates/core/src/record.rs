//! Records are the unit of replication: a typed bag of last-writer-wins fields.

use crate::hlc::Stamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const KIND_TASK: &str = "task";
pub const KIND_LIST: &str = "list";
pub const KIND_COLUMN: &str = "column";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub v: Value,
    pub t: Stamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub f: BTreeMap<String, Field>,
}

impl Record {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Record {
        Record { id: id.into(), kind: kind.into(), f: BTreeMap::new() }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.f.get(key).map(|f| &f.v)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_f64)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_i64)
    }

    pub fn is_deleted(&self) -> bool {
        self.bool("deleted")
    }

    /// Writes `value` if `stamp` is newer than what the field holds.
    /// Returns true when the record changed.
    pub fn put(&mut self, key: &str, value: Value, stamp: &Stamp) -> bool {
        match self.f.get(key) {
            Some(existing) if existing.t >= *stamp => false,
            _ => {
                self.f.insert(key.to_string(), Field { v: value, t: stamp.clone() });
                true
            }
        }
    }

    /// Field-wise last-writer-wins merge. Returns true when `self` changed.
    pub fn merge(&mut self, other: &Record) -> bool {
        let mut changed = false;
        for (key, field) in &other.f {
            changed |= self.put(key, field.v.clone(), &field.t);
        }
        changed
    }

    pub fn max_stamp(&self) -> Option<&Stamp> {
        self.f.values().map(|f| &f.t).max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::Clock;
    use serde_json::json;

    #[test]
    fn merge_is_commutative_and_idempotent() {
        let mut a = Clock::new("a");
        let mut b = Clock::new("b");
        let mut base = Record::new("t1", KIND_TASK);
        base.put("title", json!("Base"), &a.tick_at(1));

        let mut left = base.clone();
        left.put("title", json!("Left"), &a.tick_at(10));
        let mut right = base.clone();
        right.put("title", json!("Right"), &b.tick_at(5));
        right.put("notes", json!("Right notes"), &b.tick_at(11));

        let mut lr = left.clone();
        lr.merge(&right);
        let mut rl = right.clone();
        rl.merge(&left);
        assert_eq!(lr, rl);
        assert_eq!(lr.str("title"), Some("Left"));
        assert_eq!(lr.str("notes"), Some("Right notes"));
        assert!(!lr.clone().merge(&right));
    }
}
