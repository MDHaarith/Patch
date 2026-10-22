//! Clean exit on SIGTERM (logout, `systemctl`, `kill`), SIGINT (Ctrl+C) and
//! SIGHUP: the document is saved and the hub stops instead of the process
//! dying mid-write.
//!
//! The signals are blocked before any other thread exists, so every thread
//! inherits the mask, and one thread collects them with `sigwait`. That keeps
//! all real work out of async-signal context.

use std::io;

/// Must run before any thread is spawned. `on_signal` runs on the signal
/// thread; a second signal while the first is being handled exits at once.
pub fn install(on_signal: impl Fn(i32) + Send + 'static) -> io::Result<()> {
    // SAFETY: sigset_t is plain data that sigemptyset/sigaddset initialize;
    // pthread_sigmask only changes this thread's mask.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, sig);
        }
        let rc = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        set
    };
    std::thread::Builder::new().name("patch-signals".into()).spawn(move || {
        let mut handled = false;
        loop {
            let mut sig = 0;
            // SAFETY: `set` is initialized and outlives the call.
            if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                continue;
            }
            if handled {
                std::process::exit(128 + sig);
            }
            handled = true;
            on_signal(sig);
        }
    })?;
    Ok(())
}
