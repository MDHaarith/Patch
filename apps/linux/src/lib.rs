//! The UI-independent parts of Patch's Linux desktop shell: XDG paths,
//! single-instance handling, autostart, the embedded sync hub and widget
//! placement. The `patch` binary (`main.rs`) wires them to the Slint UI.

pub mod autostart;
pub mod cli;
pub mod geometry;
pub mod hosting;
pub mod instance;
pub mod paths;
pub mod state;
