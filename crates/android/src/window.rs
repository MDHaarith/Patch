//! A Slint window adapter that renders with the software renderer into a
//! [`BackBuffer`] and captures the text input's keyboard requests.

use crate::buffer::{BackBuffer, Rect};
use crate::glass::{Glass, Settings};
use crate::ime::ImeState;
use i_slint_core::input::FocusReason;
use i_slint_core::item_tree::{ItemRc, ItemWeak, ParentItemTraversalMode};
use i_slint_core::items::{InputType, TextInput, TouchArea};
use i_slint_core::window::{InputMethodRequest, WindowAdapterInternal, WindowInner};
use patch_ui::{MainWindow, Store, Theme};
use slint::platform::software_renderer::{RepaintBufferType, SoftwareRenderer};
use slint::platform::{Renderer, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use slint::{LogicalPosition, PhysicalSize, Window, WindowSize};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub struct SurfaceWindow {
    window: Window,
    renderer: SoftwareRenderer,
    size: Cell<PhysicalSize>,
    needs_redraw: Cell<bool>,
    pub ime: RefCell<ImeState>,
    selection_drag: RefCell<Option<ItemWeak>>,
    glass: RefCell<Glass>,
    glass_state: Cell<Option<(Rect, Settings)>>,
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
            selection_drag: RefCell::default(),
            glass: RefCell::default(),
            glass_state: Cell::new(None),
        })
    }

    /// Focus and select a word without releasing the finger's selection drag.
    pub fn select_word_at(self: &Rc<Self>, position: LogicalPosition) {
        let window = WindowInner::from_pub(&self.window);
        let root = ItemRc::new(window.component(), 0);
        let point = i_slint_core::lengths::LogicalPoint::new(position.x, position.y);
        let mut hit = None;
        root.visit_descendants::<()>(|item| {
            let input = item.downcast::<TextInput>();
            let editable = input.as_ref().is_some_and(|input| {
                let input = input.as_pin_ref();
                input.enabled() && !input.read_only()
            });
            let blocks = item
                .downcast::<TouchArea>()
                .is_some_and(|area| area.as_pin_ref().enabled());
            if editable || blocks {
                let geo = item.geometry();
                let rect = i_slint_core::lengths::LogicalRect::new(
                    item.map_to_window(geo.origin),
                    geo.size,
                );
                if item.is_visible() && rect.contains(point) {
                    let mut ancestor = item.parent_item(ParentItemTraversalMode::StopAtPopups);
                    while let Some(parent) = ancestor {
                        let geo = parent.geometry();
                        let clip = i_slint_core::lengths::LogicalRect::new(
                            parent.map_to_window(geo.origin),
                            geo.size,
                        );
                        if parent.borrow().as_ref().clips_children() && !clip.contains(point) {
                            return std::ops::ControlFlow::Continue(());
                        }
                        ancestor = parent.parent_item(ParentItemTraversalMode::StopAtPopups);
                    }
                    // Later touch areas (e.g. modal backdrops) cover earlier inputs.
                    hit = editable.then(|| item.clone());
                }
            }
            std::ops::ControlFlow::Continue(())
        });
        if let Some(item) = hit {
            window.set_focus_item(&item, true, FocusReason::PointerClick);
            let input = item.downcast::<TextInput>().unwrap();
            let input = input.as_pin_ref();
            let adapter: Rc<dyn WindowAdapter> = self.clone();
            let origin = item.map_to_window(item.geometry().origin);
            let local = point - origin.to_vector();
            let (offset, _) = input.byte_offset_for_position(local, &adapter, &item);
            input.set_selection_offsets(&adapter, &item, offset as i32, offset as i32);
            if input.input_type() != InputType::Password {
                input.select_word(&adapter, &item);
            }
            *self.selection_drag.borrow_mut() = Some(item.downgrade());
            self.ime.borrow_mut().show_actions();
        }
    }

    /// Keep the editor's Flickable from taking a long-press selection drag.
    pub fn drag_selection(self: &Rc<Self>, position: LogicalPosition) -> bool {
        let Some(item) = self
            .selection_drag
            .borrow()
            .as_ref()
            .and_then(ItemWeak::upgrade)
        else {
            return false;
        };
        let Some(input) = item.downcast::<TextInput>() else {
            return false;
        };
        let input = input.as_pin_ref();
        let origin = item.map_to_window(item.geometry().origin);
        let point =
            i_slint_core::lengths::LogicalPoint::new(position.x - origin.x, position.y - origin.y);
        let adapter: Rc<dyn WindowAdapter> = self.clone();
        let (offset, _) = input.byte_offset_for_position(point, &adapter, &item);
        input.set_selection_offsets(
            &adapter,
            &item,
            input.anchor_position_byte_offset(),
            offset as i32,
        );
        true
    }

    pub fn end_selection_drag(&self) {
        self.selection_drag.borrow_mut().take();
    }

    /// Render normally to learn the actual damage before touching the backdrop.
    /// Capture and final passes only invalidate the dock; unrelated frames keep
    /// the cached image. No timers or work are added to the idle path.
    pub fn draw_main(&self, frame: &mut BackBuffer, full: bool, main: &MainWindow) -> Option<Rect> {
        let dirty = self.draw(frame, full)?;
        let store = main.global::<Store>();
        let theme = main.global::<Theme>();
        if !store.get_liquid_supported()
            || !store.get_liquid_glass()
            || !theme.get_glass()
            || store.get_settings_open()
            || store.get_editor_open()
            || store.get_ime_animating()
        {
            self.glass_state.set(None);
            return Some(dirty);
        }
        let scale = self.window.scale_factor();
        let dock = store.get_dock_rect();
        let rect = Rect::new(
            (dock.x * scale).round().max(0.0) as u32,
            (dock.y * scale).round().max(0.0) as u32,
            (dock.width * scale).round().max(0.0) as u32,
            (dock.height * scale).round().max(0.0) as u32,
        )
        .clamp(frame.width(), frame.height());
        if rect.is_empty() {
            return Some(dirty);
        }
        let tint = theme.get_dock();
        let settings = Settings {
            blur: store.get_glass_blur(),
            refraction: store.get_glass_refraction(),
            chromatic: store.get_glass_chromatic(),
            depth: store.get_glass_depth(),
            tint: [tint.red(), tint.green(), tint.blue()],
            opacity: store.get_glass_opacity(),
            scale,
        };
        let margin = (settings.blur * scale * 2.0).ceil() as u32;
        let intersects = dirty.x < rect.right().saturating_add(margin)
            && dirty.right().saturating_add(margin) > rect.x
            && dirty.y < rect.bottom().saturating_add(margin)
            && dirty.bottom().saturating_add(margin) > rect.y;
        if !intersects && self.glass_state.get() == Some((rect, settings)) {
            return Some(dirty);
        }
        store.set_glass_capture(true);
        // Clear the source while capturing; the filter alternates reusable outputs.
        store.set_dock_glass(slint::Image::default());
        let stride = frame.width() as usize;
        self.renderer.render(frame.pixels_mut(), stride);
        let (w, h) = (frame.width(), frame.height());
        let image = self
            .glass
            .borrow_mut()
            .render(frame.pixels_mut(), w, h, rect, settings);
        store.set_dock_glass(slint::Image::from_rgba8(image));
        store.set_glass_capture(false);
        let region = self.renderer.render(frame.pixels_mut(), w as usize);
        self.glass_state.set(Some((rect, settings)));
        // These property changes have already been painted, not a new animation.
        self.needs_redraw.set(false);
        Some(
            dirty
                .union(Rect::bounds_of(&region))
                .union(rect)
                .clamp(w, h),
        )
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
