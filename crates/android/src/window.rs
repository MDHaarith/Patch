//! A Slint window adapter that renders with the software renderer into a
//! [`BackBuffer`] and captures the text input's keyboard requests.

use crate::buffer::{BackBuffer, Rect};
use crate::ime::ImeState;
use i_slint_core::window::{InputMethodRequest, WindowAdapterInternal};
use slint::platform::software_renderer::{RepaintBufferType, SoftwareRenderer};
use slint::platform::{Renderer, WindowAdapter, WindowEvent};
use slint::{PhysicalSize, Window, WindowSize};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub struct SurfaceWindow {
    window: Window,
    renderer: SoftwareRenderer,
    size: Cell<PhysicalSize>,
    needs_redraw: Cell<bool>,
    pub ime: RefCell<ImeState>,
}

impl SurfaceWindow {
    pub fn new() -> Rc<SurfaceWindow> {
        Rc::new_cyclic(|w: &Weak<SurfaceWindow>| SurfaceWindow {
            window: Window::new(w.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(
                RepaintBufferType::ReusedBuffer,
            ),
            size: Cell::new(PhysicalSize::default()),
            needs_redraw: Cell::new(false),
            ime: RefCell::default(),
        })
    }

    /// Renders into `frame` if Slint asked for a redraw, or everything when
    /// `full` (new size, new scale, fresh buffer). Returns the repainted area.
    pub fn draw(&self, frame: &mut BackBuffer, full: bool) -> Option<Rect> {
        if !self.needs_redraw.replace(false) && !full {
            return None;
        }
        if frame.is_empty() {
            return None;
        }
        let (w, h) = (frame.width(), frame.height());
        if full {
            // Switching the buffer type drops the partial-rendering cache,
            // which is exactly a "repaint everything" request.
            self.renderer
                .set_repaint_buffer_type(RepaintBufferType::NewBuffer);
            self.renderer.render(frame.pixels_mut(), w as usize);
            self.renderer
                .set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
            return Some(Rect::full(w, h));
        }
        let region = self.renderer.render(frame.pixels_mut(), w as usize);
        Some(Rect::bounds_of(&region).clamp(w, h)).filter(|r| !r.is_empty())
    }
}

impl WindowAdapter for SurfaceWindow {
    fn window(&self) -> &Window {
        &self.window
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn set_size(&self, size: WindowSize) {
        let scale = self.window.scale_factor();
        self.size.set(size.to_physical(scale));
        self.window.dispatch_event(WindowEvent::Resized {
            size: size.to_logical(scale),
        });
    }

    fn request_redraw(&self) {
        self.needs_redraw.set(true);
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn internal(&self, _: i_slint_core::InternalToken) -> Option<&dyn WindowAdapterInternal> {
        Some(self)
    }
}

impl WindowAdapterInternal for SurfaceWindow {
    fn input_method_request(&self, request: InputMethodRequest) {
        self.ime.borrow_mut().request(request);
    }
}
