//! Patch for Android, as a native library loaded by a tiny Java shell.
//!
//! The whole UI — the app screen and the floating "display over other apps"
//! panel — runs in one process on one Rust thread with one Slint platform.
//! Each Java `SurfaceView` hands us its `Surface`; we render the matching
//! Slint window with the software renderer into a back buffer and copy the
//! changed part into the surface.
//!
//! ```text
//!  Java main thread ──JNI──▶ mpsc ──▶ UI thread (runtime.rs) ──▶ ANativeWindow
//!        ▲                                  │
//!        └────── PatchNative statics ◀──────┘  (Bridge: IME, overlay, notify…)
//! ```

pub mod buffer;
pub mod host;
pub mod ime;
pub mod input;
pub mod platform;
pub mod runtime;
pub mod window;

#[cfg(target_os = "android")]
mod android;
