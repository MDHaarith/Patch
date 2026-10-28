//! JNI entry points of `com.mdhaarith.patch.PatchNative`.
//!
//! They run on Java threads and only translate their arguments into a
//! [`Msg`] for the UI thread; the two that must be synchronous
//! (`surfaceDestroyed`, `onPause`) wait for its reply with a timeout.

mod bridge;
mod logcat;
mod native_window;

use crate::ime::ImeOp;
use crate::input::TouchAction;
use crate::runtime::{Kind, Msg, Runtime};
use bridge::JniBridge;
use jni::objects::{GlobalRef, JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jint, JNI_TRUE};
use jni::{JNIEnv, JavaVM};
use native_window::{NativeWindow, FORMAT_RGBA, FORMAT_RGBX};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::OnceLock;
use std::time::Duration;

struct Globals {
    vm: JavaVM,
    /// `PatchNative`. Native threads can't `FindClass` app classes (they get
    /// the system class loader), so it's resolved once here.
    class: GlobalRef,
    /// The application context, kept alive for the life of the process.
    _context: GlobalRef,
    tx: Sender<Msg>,
}

static GLOBALS: OnceLock<Globals> = OnceLock::new();

fn send(msg: Msg) {
    if let Some(g) = GLOBALS.get() {
        let _ = g.tx.send(msg);
    }
}

/// Sends a message carrying a reply channel and waits (bounded) for it.
fn send_and_wait(
    make: impl FnOnce(std::sync::mpsc::SyncSender<()>) -> Msg,
    timeout: Duration,
    what: &str,
) {
    let (done, wait) = sync_channel(1);
    send(make(done));
    if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(timeout) {
        log::warn!("{what}: UI thread didn't answer in {timeout:?}");
    }
}

fn kind(kind: jint) -> Option<Kind> {
    Kind::from_java(kind)
}

fn string(env: &mut JNIEnv, s: &JString) -> String {
    if s.is_null() {
        return String::new();
    }
    env.get_string(s).map(String::from).unwrap_or_default()
}

/// Starts the UI thread. Idempotent: the activity, the overlay service and
/// the boot receiver all call it, whichever runs first wins.
#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeInit(
    mut env: JNIEnv,
    class: JClass,
    context: JObject,
    files_dir: JString,
    density: jfloat,
) {
    if GLOBALS.get().is_some() {
        send(Msg::Density(density));
        return;
    }
    logcat::init();
    let data_dir = PathBuf::from(string(&mut env, &files_dir));
    let globals = (|| -> jni::errors::Result<_> {
        Ok((
            env.get_java_vm()?,
            env.new_global_ref(&class)?,
            env.new_global_ref(&context)?,
        ))
    })();
    let (vm, class, context) = match globals {
        Ok(g) => g,
        Err(e) => {
            log::error!("nativeInit: {e}");
            return;
        }
    };
    let (tx, rx) = channel();
    if GLOBALS
        .set(Globals {
            vm,
            class,
            _context: context,
            tx: tx.clone(),
        })
        .is_err()
    {
        return;
    }
    log::info!("starting, data in {}", data_dir.display());
    let spawned = std::thread::Builder::new()
        .name("patch-ui".into())
        .stack_size(8 << 20)
        .spawn(move || ui_thread(rx, tx, data_dir, density));
    if let Err(e) = spawned {
        log::error!("can't start the UI thread: {e}");
    }
}

fn ui_thread(rx: Receiver<Msg>, tx: Sender<Msg>, data_dir: PathBuf, density: f32) {
    let Some(g) = GLOBALS.get() else { return };
    if let Err(e) = g.vm.attach_current_thread_permanently() {
        log::error!("can't attach the UI thread to the VM: {e}");
        return;
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bridge = Rc::new(JniBridge::new(&g.vm, &g.class));
        match Runtime::new(data_dir, density, bridge, tx) {
            Ok(runtime) => runtime.run(rx),
            Err(e) => log::error!("{e}"),
        }
    }));
    if outcome.is_err() {
        // The panic hook has logged it. A frozen UI would be worse than a
        // crash: let Android restart us cleanly.
        std::process::abort();
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeSurfaceChanged(
    env: JNIEnv,
    _: JClass,
    kind_: jint,
    surface: JObject,
    width: jint,
    height: jint,
) {
    let Some(kind) = kind(kind_) else { return };
    let format = if kind == Kind::Overlay {
        FORMAT_RGBA
    } else {
        FORMAT_RGBX
    };
    let Some(window) = NativeWindow::from_surface(&env, &surface, format) else {
        log::warn!("{kind:?}: no native window for the surface");
        return;
    };
    send(Msg::Surface {
        kind,
        surface: Box::new(window),
        width: width.max(0) as u32,
        height: height.max(0) as u32,
    });
}

/// Java must not return from `surfaceDestroyed` while we still draw into it.
#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeSurfaceDestroyed(
    _: JNIEnv,
    _: JClass,
    kind_: jint,
) {
    let Some(kind) = kind(kind_) else { return };
    send_and_wait(
        |done| Msg::SurfaceLost { kind, done },
        Duration::from_secs(2),
        "surfaceDestroyed",
    );
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeRedrawNeeded(
    _: JNIEnv,
    _: JClass,
    kind_: jint,
) {
    if let Some(kind) = kind(kind_) {
        send(Msg::RedrawNeeded(kind));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeViewGone(
    _: JNIEnv,
    _: JClass,
    kind_: jint,
) {
    if let Some(kind) = kind(kind_) {
        send(Msg::ViewGone(kind));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeTouch(
    _: JNIEnv,
    _: JClass,
    kind_: jint,
    action: jint,
    x: jfloat,
    y: jfloat,
) {
    if let (Some(kind), Some(action)) = (kind(kind_), TouchAction::from_java(action)) {
        send(Msg::Touch { kind, action, x, y });
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeKey(
    _: JNIEnv,
    _: JClass,
    kind_: jint,
    code: jint,
    unicode: jint,
    down: jboolean,
    seq: jint,
) {
    if let Some(kind) = kind(kind_) {
        send(Msg::Key {
            kind,
            code,
            unicode: unicode.max(0) as u32,
            down: down == JNI_TRUE,
            seq,
        });
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeIme(
    mut env: JNIEnv,
    _: JClass,
    kind_: jint,
    op: jint,
    text: JString,
    a: jint,
    b: jint,
    seq: jint,
) {
    let Some(kind) = kind(kind_) else { return };
    let text = string(&mut env, &text);
    if let Some(op) = ImeOp::from_java(op, text, a, b) {
        send(Msg::Ime { kind, op, seq });
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeBack(_: JNIEnv, _: JClass) {
    send(Msg::Back);
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeInsets(
    _: JNIEnv,
    _: JClass,
    top: jfloat,
    bottom: jfloat,
) {
    send(Msg::Insets { top, bottom });
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeDensity(
    _: JNIEnv,
    _: JClass,
    density: jfloat,
) {
    send(Msg::Density(density));
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativePetPlay(
    _: JNIEnv,
    _: JClass,
    state: jint,
) {
    send(Msg::PetPlay(state));
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeOverlayPosition(
    _: JNIEnv,
    _: JClass,
    x: jfloat,
    y: jfloat,
) {
    send(Msg::OverlayPosition { x, y });
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeOverlayAllowed(
    _: JNIEnv,
    _: JClass,
    allowed: jboolean,
) {
    send(Msg::OverlayAllowed(allowed == JNI_TRUE));
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeOverlayClosed(
    _: JNIEnv,
    _: JClass,
) {
    send(Msg::OverlayClosed);
}

/// Saves before returning: after `onPause` the process may be killed.
#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativePause(_: JNIEnv, _: JClass) {
    send_and_wait(
        |done| Msg::Pause(Some(done)),
        Duration::from_millis(800),
        "onPause",
    );
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeOpenTask(
    mut env: JNIEnv,
    _: JClass,
    id: JString,
) {
    let id = string(&mut env, &id);
    if !id.is_empty() {
        send(Msg::OpenTask(id));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeCompleteTask(
    mut env: JNIEnv,
    _: JClass,
    id: JString,
) {
    let id = string(&mut env, &id);
    if !id.is_empty() {
        send_and_wait(|done| Msg::CompleteTask(id, Some(done)), Duration::from_secs(8), "completeTask");
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeTaskOpen(
    mut env: JNIEnv,
    _: JClass,
    id: JString,
) -> jboolean {
    let id = string(&mut env, &id);
    let (reply, wait) = sync_channel(1);
    send(Msg::Invoke(Box::new(move || {
        let _ = reply.send(patch_ui::App::current_task_open(&id));
    })));
    wait.recv_timeout(Duration::from_secs(8))
        .ok()
        .flatten()
        .unwrap_or(true) as jboolean
}

/// The widget's "+": open the editor on a new task.
#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeNewTask(_env: JNIEnv, _: JClass) {
    send(Msg::NewTask);
}

/// A QR code scanned for pairing.
#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeScanned(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) {
    send(Msg::Scanned(string(&mut env, &text)));
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeScanStopped(_: JNIEnv, _: JClass) {
    send(Msg::ScanStopped);
}

#[no_mangle]
pub extern "system" fn Java_com_mdhaarith_patch_PatchNative_nativeSyncNow(_: JNIEnv, _: JClass) {
    send(Msg::SyncNow);
}
