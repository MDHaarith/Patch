//! The floating widget: the shared `OverlayWindow` (frameless, transparent,
//! always on top) as a 340×500 panel or a 60×60 bubble.
//!
//! Frameless windows have no title bar to drag, so the widget starts a
//! window-manager move itself: from the panel's header (left of its buttons)
//! or anywhere on the bubble. A press there is held back from Slint until it is clear whether
//! it is a click (released in place: the press and release are then
//! delivered, so the header buttons and the bubble's tap still work) or a
//! drag (the pointer moved a few pixels: `drag_window()` hands the pointer to
//! the window manager). Forwarding the press right away would leave Slint
//! waiting for a release the window manager swallows during the move.

use crate::screen::{Monitor, Screen};
use patch_linux::geometry::{default_rect, resize_anchored, Rect};
use patch_linux::state::WidgetPlace;
use patch_ui::{OverlayWindow, Store};
use slint::platform::{PointerEventButton, WindowEvent as SlintEvent};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
use slint::winit_030::winit::window::WindowLevel;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::Cell;
use std::rc::Rc;

pub const PANEL: (f32, f32) = (340.0, 500.0);
pub const BUBBLE: (f32, f32) = (60.0, 60.0);
/// The panel's header row (clock and date); see `OverlayPanel`.
const HEADER_HEIGHT: f64 = 52.0;
/// The header's "open" and "collapse" buttons on the right (12px padding,
/// two 32px buttons, spacing), which stay plain buttons.
const HEADER_BUTTONS: f64 = 84.0;
/// Logical pixels the pointer must travel before a press becomes a drag.
const DRAG_SLOP: f64 = 4.0;

pub fn size_for(expanded: bool) -> (f32, f32) {
    if expanded {
        PANEL
    } else {
        BUBBLE
    }
}

fn physical(size: (f32, f32), scale: f64) -> (i32, i32) {
    ((f64::from(size.0) * scale).round() as i32, (f64::from(size.1) * scale).round() as i32)
}

pub struct Widget {
    window: OverlayWindow,
}

#[derive(Default)]
struct Pointer {
    /// Last cursor position, physical pixels relative to the window.
    at: Cell<Option<PhysicalPosition<f64>>>,
    /// A left press in the drag zone that Slint hasn't seen yet.
    held: Cell<Option<PhysicalPosition<f64>>>,
}

impl Widget {
    /// Wraps an overlay already attached to the `App`. `on_moved` gets the new
    /// top-left corner (physical pixels) whenever the window manager moves it.
    pub fn new(window: OverlayWindow, on_moved: impl Fn(i32, i32) + 'static) -> Widget {
        let pointer = Rc::new(Pointer::default());
        let weak = window.as_weak();
        window.window().on_winit_window_event(move |w, event| {
            let scale = f64::from(w.scale_factor());
            match event {
                WindowEvent::CursorMoved { position, .. } => {
                    pointer.at.set(Some(*position));
                    if let Some(start) = pointer.held.get() {
                        let (dx, dy) = (position.x - start.x, position.y - start.y);
                        if dx.hypot(dy) / scale > DRAG_SLOP {
                            pointer.held.set(None);
                            w.with_winit_window(|ww| {
                                if let Err(e) = ww.drag_window() {
                                    eprintln!("patch: can't move the widget: {e}");
                                }
                            });
                        }
                    }
                }
                WindowEvent::CursorLeft { .. } => pointer.at.set(None),
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                    let expanded = weak.upgrade().is_some_and(|o| o.global::<Store>().get_overlay_expanded());
                    if let Some(at) = pointer.at.get() {
                        let width = f64::from(w.size().width) / scale;
                        let (x, y) = (at.x / scale, at.y / scale);
                        if !expanded || (y < HEADER_HEIGHT && x < width - HEADER_BUTTONS) {
                            pointer.held.set(Some(at));
                            return EventResult::PreventDefault;
                        }
                    }
                }
                WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                    if let Some(at) = pointer.held.take() {
                        // A click after all: Slint gets the press now and the
                        // release right after, through normal handling.
                        w.dispatch_event(SlintEvent::PointerPressed {
                            position: LogicalPosition::new((at.x / scale) as f32, (at.y / scale) as f32),
                            button: PointerEventButton::Left,
                        });
                    }
                }
                WindowEvent::Moved(p) => on_moved(p.x, p.y),
                _ => {}
            }
            EventResult::Propagate
        });
        Widget { window }
    }

    pub fn window(&self) -> &OverlayWindow {
        &self.window
    }

    pub fn expanded(&self) -> bool {
        self.window.global::<Store>().get_overlay_expanded()
    }

    /// Sizes and positions the window before it is first shown. On X11 it
    /// goes back to `saved` (re-anchored if it was saved in the other mode),
    /// or to the top-right corner the first time. Wayland places it itself.
    pub fn place(&self, screen: &Screen, saved: Option<WidgetPlace>) -> Option<WidgetPlace> {
        let expanded = self.expanded();
        let (w, h) = size_for(expanded);
        self.window.window().set_size(LogicalSize::new(w, h));
        if screen.wayland {
            return None;
        }
        let rect = saved
            .and_then(|s| {
                // Off-screen (monitor unplugged since): fall back to the default spot.
                let (lw, lh) = physical(size_for(s.expanded), 1.0);
                let monitor = screen.monitor_of(&Rect::new(s.x, s.y, lw, lh))?;
                let (sw, sh) = physical(size_for(s.expanded), monitor.scale);
                let old = Rect::new(s.x, s.y, sw, sh);
                let (nw, nh) = physical((w, h), monitor.scale);
                Some(if s.expanded == expanded {
                    Rect::new(old.x, old.y, nw, nh).clamped_to(&monitor.rect)
                } else {
                    resize_anchored(old, nw, nh, &monitor.rect)
                })
            })
            .or_else(|| {
                let m = screen.primary?;
                let (pw, ph) = physical((w, h), m.scale);
                Some(default_rect(pw, ph, &m.rect, m.scale as f32))
            })?;
        self.window.window().set_position(slint::PhysicalPosition::new(rect.x, rect.y));
        Some(WidgetPlace { x: rect.x, y: rect.y, expanded })
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn hide(&self) {
        let _ = self.window.hide();
    }

    /// Current spot on X11; `None` on Wayland or before the window exists.
    pub fn place_now(&self) -> Option<WidgetPlace> {
        let expanded = self.expanded();
        self.window.window().with_winit_window(|ww| ww.outer_position().ok()).flatten().map(|p| WidgetPlace {
            x: p.x,
            y: p.y,
            expanded,
        })
    }

    /// Re-applies what the window manager may have dropped while mapping the
    /// window. winit asks for "always on top" when it creates the window, but
    /// EWMH window managers ignore state requests for windows that aren't
    /// mapped yet; some also ignore the initial position.
    pub fn settle(&self, place: Option<WidgetPlace>) {
        self.window.window().with_winit_window(|ww| ww.set_window_level(WindowLevel::AlwaysOnTop));
        if let Some(place) = place {
            if self.place_now().is_some_and(|now| (now.x, now.y) != (place.x, place.y)) {
                self.window.window().set_position(slint::PhysicalPosition::new(place.x, place.y));
            }
        }
    }

    /// Switches between bubble and panel size. On X11 the corner nearest the
    /// screen edge stays where it is; returns the new spot to remember.
    pub fn resize(&self, expanded: bool) -> Option<WidgetPlace> {
        let (w, h) = size_for(expanded);
        let target = self
            .window
            .window()
            .with_winit_window(|ww| {
                let pos = ww.outer_position().ok()?;
                let size = ww.outer_size();
                let old = Rect::new(pos.x, pos.y, size.width as i32, size.height as i32);
                let monitor = ww
                    .current_monitor()
                    .map(|m| Monitor::from_handle(&m))
                    .unwrap_or(Monitor { rect: old, scale: ww.scale_factor() });
                let (nw, nh) = physical((w, h), ww.scale_factor());
                Some(resize_anchored(old, nw, nh, &monitor.rect))
            })
            .flatten();
        self.window.window().set_size(LogicalSize::new(w, h));
        let rect = target?;
        self.window.window().set_position(slint::PhysicalPosition::new(rect.x, rect.y));
        Some(WidgetPlace { x: rect.x, y: rect.y, expanded })
    }
}
