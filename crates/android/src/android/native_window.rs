//! A Java `Surface` as an `ANativeWindow` we can lock and fill from the CPU.

use crate::buffer::{BackBuffer, Rect};
use crate::runtime::Surface;
use jni::objects::JObject;
use jni::JNIEnv;
use std::ffi::c_void;
use std::ptr::NonNull;

#[repr(C)]
struct ANativeWindow {
    _private: [u8; 0],
}

#[repr(C)]
struct ANativeWindowBuffer {
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    bits: *mut c_void,
    reserved: [u32; 6],
}

#[repr(C)]
struct ARect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "android")]
extern "C" {
    fn ANativeWindow_fromSurface(
        env: *mut jni::sys::JNIEnv,
        surface: jni::sys::jobject,
    ) -> *mut ANativeWindow;
    fn ANativeWindow_release(window: *mut ANativeWindow);
    fn ANativeWindow_setBuffersGeometry(
        window: *mut ANativeWindow,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
    fn ANativeWindow_lock(
        window: *mut ANativeWindow,
        buffer: *mut ANativeWindowBuffer,
        dirty: *mut ARect,
    ) -> i32;
    fn ANativeWindow_unlockAndPost(window: *mut ANativeWindow) -> i32;
}

/// `WINDOW_FORMAT_RGBA_8888`: the overlay needs alpha around the bubble.
pub const FORMAT_RGBA: i32 = 1;
/// `WINDOW_FORMAT_RGBX_8888`: same memory layout, composited as opaque.
pub const FORMAT_RGBX: i32 = 2;

pub struct NativeWindow {
    window: NonNull<ANativeWindow>,
    warned_format: bool,
}

// SAFETY: ANativeWindow is reference counted and may be used from any thread;
// we only ever use it from one thread at a time.
unsafe impl Send for NativeWindow {}

impl NativeWindow {
    pub fn from_surface(env: &JNIEnv, surface: &JObject, format: i32) -> Option<NativeWindow> {
        if surface.is_null() {
            return None;
        }
        // SAFETY: `surface` is a live android.view.Surface local reference;
        // the returned window carries its own reference (released on drop).
        let window =
            NonNull::new(unsafe { ANativeWindow_fromSurface(env.get_raw(), surface.as_raw()) })?;
        // 0×0 keeps the surface's own size; only the pixel format changes.
        // SAFETY: `window` is valid.
        let err = unsafe { ANativeWindow_setBuffersGeometry(window.as_ptr(), 0, 0, format) };
        if err != 0 {
            log::warn!("ANativeWindow_setBuffersGeometry failed: {err}");
        }
        Some(NativeWindow {
            window,
            warned_format: false,
        })
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { ANativeWindow_release(self.window.as_ptr()) };
    }
}

impl Surface for NativeWindow {
    fn present(&mut self, frame: &BackBuffer, rect: Rect) -> Result<(), String> {
        let mut dirty = ARect {
            left: rect.x as i32,
            top: rect.y as i32,
            right: rect.right() as i32,
            bottom: rect.bottom() as i32,
        };
        let mut buffer = ANativeWindowBuffer {
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            bits: std::ptr::null_mut(),
            reserved: [0; 6],
        };
        // SAFETY: valid window and out-pointers.
        let err = unsafe { ANativeWindow_lock(self.window.as_ptr(), &mut buffer, &mut dirty) };
        if err != 0 {
            return Err(format!("ANativeWindow_lock failed: {err}"));
        }
        // The surface carries the previous frame over outside `dirty`, and
        // widens `dirty` to whatever it couldn't carry over.
        let area = Rect::new(
            dirty.left.max(0) as u32,
            dirty.top.max(0) as u32,
            (dirty.right - dirty.left.max(0)).max(0) as u32,
            (dirty.bottom - dirty.top.max(0)).max(0) as u32,
        );
        if (buffer.format == FORMAT_RGBA || buffer.format == FORMAT_RGBX) && !buffer.bits.is_null()
        {
            // SAFETY: a locked buffer is `stride * height` 4-byte pixels.
            unsafe {
                frame.copy_to(
                    area,
                    buffer.bits.cast(),
                    buffer.width.max(0) as u32,
                    buffer.height.max(0) as u32,
                    buffer.stride.max(0) as u32,
                )
            };
        } else if !self.warned_format {
            self.warned_format = true;
            log::error!("unsupported surface format {}", buffer.format);
        }
        // SAFETY: the window is locked.
        let err = unsafe { ANativeWindow_unlockAndPost(self.window.as_ptr()) };
        if err != 0 {
            return Err(format!("ANativeWindow_unlockAndPost failed: {err}"));
        }
        Ok(())
    }
}
