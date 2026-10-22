//! Patch core: the data model shared by the desktop app, the Android app and
//! the sync hub.
//!
//! Every object (task, list, board column) is a [`Record`]: a map of fields
//! where each field carries the [`Stamp`] of the write that produced it.
//! Merging two copies of a record keeps, per field, the value with the
//! highest stamp. That makes merging commutative, associative and
//! idempotent, so devices can edit offline and converge once they sync.

pub mod doc;
pub mod hlc;
pub mod id;
pub mod model;
pub mod quick;
pub mod record;
pub mod recur;
pub mod time;

pub use doc::Doc;
pub use hlc::{Clock, Stamp};
pub use model::{CheckItem, Column, List, Repeat, Task};
pub use record::{Field, Record};
