//! [`Bridge`] over JNI: static methods of `com.mdhaarith.patch.PatchNative`.
//! Only ever used on the UI thread, which is attached to the VM for good.

use crate::ime::{ImeMode, ImeSnapshot};
use crate::runtime::{Bridge, Kind};
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};

pub struct JniBridge {
    vm: &'static JavaVM,
    class: &'static GlobalRef,
}

impl JniBridge {
    pub fn new(vm: &'static JavaVM, class: &'static GlobalRef) -> JniBridge {
        JniBridge { vm, class }
    }

    /// Runs `f` in a local reference frame: this thread never returns to
    /// Java, so local references would otherwise pile up forever. A Java
    /// exception is logged and cleared instead of poisoning the next call.
    fn with_env<R>(
        &self,
        what: &str,
        f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<R>,
    ) -> Option<R> {
        let mut env = match self.vm.get_env() {
            Ok(env) => env,
            Err(e) => {
                log::error!("{what}: no JNI environment: {e}");
                return None;
            }
        };
        let result = env.with_local_frame(8, |env| f(env));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        match result {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("{what} failed: {e}");
                None
            }
        }
    }

    fn call_void(
        &self,
        name: &str,
        sig: &str,
        args: impl for<'l> FnOnce(&mut JNIEnv<'l>) -> jni::errors::Result<Vec<Arg<'l>>>,
    ) {
        self.with_env(name, |env| {
            let args = args(env)?;
            let values: Vec<JValue> = args.iter().map(Arg::value).collect();
            env.call_static_method(self.class, name, sig, &values)?;
            Ok(())
        });
    }
}

/// An argument whose local reference (if any) lives until the call is made.
enum Arg<'local> {
    Bool(bool),
    Int(i32),
    Obj(JObject<'local>),
}

impl Arg<'_> {
    fn value(&self) -> JValue<'_, '_> {
        match self {
            Arg::Bool(b) => JValue::Bool(*b as u8),
            Arg::Int(i) => JValue::Int(*i),
            Arg::Obj(o) => JValue::Object(o),
        }
    }
}

fn string<'local>(env: &mut JNIEnv<'local>, s: &str) -> jni::errors::Result<Arg<'local>> {
    Ok(Arg::Obj(env.new_string(s)?.into()))
}

impl Bridge for JniBridge {
    fn set_overlay(&self, on: bool) -> bool {
        self.with_env("setOverlay", |env| {
            env.call_static_method(self.class, "setOverlay", "(Z)Z", &[JValue::Bool(on as u8)])?
                .z()
        })
        .unwrap_or(false)
    }

    fn overlay_allowed(&self) -> bool {
        self.with_env("overlayAllowed", |env| {
            env.call_static_method(self.class, "overlayAllowed", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    fn set_overlay_boot(&self, on: bool) {
        self.call_void("setOverlayBoot", "(Z)V", |_| Ok(vec![Arg::Bool(on)]));
    }

    fn overlay_expand(&self, expanded: bool) {
        self.call_void("overlayExpand", "(Z)V", |_| Ok(vec![Arg::Bool(expanded)]));
    }

    fn open_app(&self, task_id: Option<&str>) {
        self.call_void("openApp", "(Ljava/lang/String;)V", |env| {
            Ok(vec![match task_id {
                Some(id) => string(env, id)?,
                None => Arg::Obj(JObject::null()),
            }])
        });
    }

    fn notify(&self, title: &str, body: &str) {
        self.call_void(
            "postNotification",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            |env| Ok(vec![string(env, title)?, string(env, body)?]),
        );
    }

    fn schedule_reminders(&self, json: &str) {
        self.call_void("scheduleReminders", "(Ljava/lang/String;)V", |env| Ok(vec![string(env, json)?]));
    }

    fn publish_snapshot(&self, json: &str) {
        self.call_void("publishSnapshot", "(Ljava/lang/String;)V", |env| Ok(vec![string(env, json)?]));
    }

    fn set_clipboard(&self, text: &str) {
        self.call_void("copyText", "(Ljava/lang/String;)V", |env| {
            Ok(vec![string(env, text)?])
        });
    }

    fn clipboard(&self) -> Option<String> {
        self.with_env("clipboardText", |env| {
            let obj = env
                .call_static_method(self.class, "clipboardText", "()Ljava/lang/String;", &[])?
                .l()?;
            if obj.is_null() {
                return Ok(None);
            }
            let s: String = env.get_string(&JString::from(obj))?.into();
            Ok(Some(s))
        })
        .flatten()
    }

    fn ime(&self, kind: Kind, mode: ImeMode, state: &ImeSnapshot, seq: i32) {
        self.call_void("imeState", "(IIILjava/lang/String;IIIIII)V", |env| {
            Ok(vec![
                Arg::Int(kind.to_java()),
                Arg::Int(mode as i32),
                Arg::Int(seq),
                string(env, &state.text)?,
                Arg::Int(state.sel_start),
                Arg::Int(state.sel_end),
                Arg::Int(state.comp_start),
                Arg::Int(state.comp_end),
                Arg::Int(state.input_type),
                Arg::Int(state.action),
            ])
        });
    }

    fn frame_done(&self, kind: Kind) {
        self.call_void("frameDone", "(I)V", |_| Ok(vec![Arg::Int(kind.to_java())]));
    }

    fn back_result(&self, consumed: bool) {
        self.call_void("backResult", "(Z)V", |_| Ok(vec![Arg::Bool(consumed)]));
    }

    fn set_look(&self, look: i32) {
        self.call_void("setLook", "(I)V", |_| Ok(vec![Arg::Int(look)]));
    }

    fn scan_qr(&self) {
        self.call_void("scanQr", "()V", |_| Ok(vec![]));
    }

    fn battery_unrestricted(&self) -> bool {
        self.with_env("batteryUnrestricted", |env| {
            env.call_static_method(self.class, "batteryUnrestricted", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    fn request_battery_unrestricted(&self) {
        self.call_void("requestBatteryUnrestricted", "()V", |_| Ok(vec![]));
    }
}
