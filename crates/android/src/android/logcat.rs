//! `log` → logcat, and panics → logcat.

use std::ffi::{c_char, c_int, CString};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

const TAG: &[u8] = b"Patch\0";

struct Logcat;

impl log::Log for Logcat {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let prio = match record.level() {
            log::Level::Error => 6,
            log::Level::Warn => 5,
            log::Level::Info => 4,
            log::Level::Debug => 3,
            log::Level::Trace => 2,
        };
        let text = match record.module_path() {
            Some(module) if record.level() >= log::Level::Debug => {
                format!("{module}: {}", record.args())
            }
            _ => record.args().to_string(),
        };
        write(prio, &text);
    }

    fn flush(&self) {}
}

fn write(prio: c_int, text: &str) {
    let text = CString::new(text.replace('\0', "\\0")).unwrap_or_default();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { __android_log_write(prio, TAG.as_ptr().cast(), text.as_ptr()) };
}

pub fn init() {
    static LOGGER: Logcat = Logcat;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(if cfg!(debug_assertions) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
    // Without this a Rust panic only shows up as an anonymous abort.
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        write(
            7,
            &format!("panic on thread '{}': {info}", thread.name().unwrap_or("?")),
        );
    }));
}
