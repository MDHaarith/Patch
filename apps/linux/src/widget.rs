//! Fixed overlay windows: a 60×60 pet that stays above apps and a 352×352
//! desk tile board that sits below them (or floats when requested). The shell
//! sets their layers; neither follows the shared overlay's expanded state.
//!
//! Both windows can be dragged anywhere. A press is held back from Slint
//! until it is clear whether it is a click (press and release are delivered
//! together, so tapping items still works) or a drag (`drag_window()` hands
//! the pointer to the window manager). This avoids leaving Slint waiting for
//! a release the window manager swallows during the move.

use crate::screen::{Monitor, Screen};
use patch_linux::geometry::{default_rect, Rect};
use patch_linux::state::WidgetPlace;
use patch_ui::OverlayWindow;
use slint::platform::{PointerEventButton, WindowEvent as SlintEvent};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::Cell;
use std::rc::Rc;

pub const BUBBLE: (f32, f32) = (60.0, 60.0);
pub const DESK: (f32, f32) = (352.0, 352.0);
/// Logical pixels the pointer must travel before a press becomes a drag.
const DRAG_SLOP: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Pet = 1,
    Desk = 2,
}

impl Mode {
    fn size(self) -> (f32, f32) {
        match self {
            Self::Pet => BUBBLE,
            Self::Desk => DESK,
        }
    }
}

fn default_place(mode: Mode, monitor: Monitor) -> Rect {
    let (w, h) = physical(mode.size(), monitor.scale);
    let mut rect = default_rect(w, h, &monitor.rect, monitor.scale as f32);
    if mode == Mode::Desk {
        rect.y += (f64::from(BUBBLE.1 + 24.0) * monitor.scale).round() as i32;
    }
    rect.clamped_to(&monitor.rect)
}

fn physical(size: (f32, f32), scale: f64) -> (i32, i32) {
    ((f64::from(size.0) * scale).round() as i32, (f64::from(size.1) * scale).round() as i32)
}

pub struct Widget {
    window: OverlayWindow,
    mode: Mode,
}

#[derive(Default)]
struct Pointer {
    /// Last cursor position, physical pixels relative to the window.
    at: Cell<Option<PhysicalPosition<f64>>>,
    /// A left press in the drag zone that Slint hasn't seen yet.
    held: Cell<Option<PhysicalPosition<f64>>>,
}

impl Widget {
    /// Sets up a fixed overlay for attachment to the `App`. `on_moved` gets the new
    /// top-left corner (physical pixels) whenever the window manager moves it,
    /// `on_focus` focus changes.
    pub fn new(
        window: OverlayWindow,
        mode: Mode,
        on_moved: impl Fn(i32, i32) + 'static,
        on_focus: impl Fn(bool) + 'static,
    ) -> Widget {
        window.set_mode(mode as i32);
        let pointer = Rc::new(Pointer::default());
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
                    if let Some(at) = pointer.at.get() {
                        pointer.held.set(Some(at));
                        return EventResult::PreventDefault;
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
                WindowEvent::Focused(f) => on_focus(*f),
                _ => {}
            }
            EventResult::Propagate
        });
        Widget { window, mode }
    }

    pub fn window(&self) -> &OverlayWindow {
        &self.window
    }

    /// Sizes and positions the window before it is first shown. On X11 it
    /// returns to its saved corner, clamped to its monitor. New windows go
    /// at the top right, with the desk below the pet. Wayland places them itself.
    pub fn place(&self, screen: &Screen, saved: Option<WidgetPlace>) -> Option<WidgetPlace> {
        let (w, h) = self.mode.size();
        self.window.window().set_size(LogicalSize::new(w, h));
        if screen.wayland {
            return None;
        }
        let rect = saved
            .and_then(|s| {
                // Off-screen (monitor unplugged since): fall back to the default spot.
                let (lw, lh) = physical((w, h), 1.0);
                let monitor = screen.monitor_of(&Rect::new(s.x, s.y, lw, lh))?;
                let (pw, ph) = physical((w, h), monitor.scale);
                Some(Rect::new(s.x, s.y, pw, ph).clamped_to(&monitor.rect))
            })
            .or_else(|| screen.primary.map(|m| default_place(self.mode, m)))?;
        self.window.window().set_position(slint::PhysicalPosition::new(rect.x, rect.y));
        Some(WidgetPlace { x: rect.x, y: rect.y, expanded: false })
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn hide(&self) {
        let _ = self.window.hide();
    }

    /// Current spot on X11; `None` on Wayland or before the window exists.
    pub fn place_now(&self) -> Option<WidgetPlace> {
        self.window.window().with_winit_window(|ww| ww.outer_position().ok()).flatten().map(|p| WidgetPlace {
            x: p.x,
            y: p.y,
            expanded: false,
        })
    }

    /// Re-applies the position some window managers ignore while mapping the
    /// window. (The shell re-applies the layer.)
    pub fn settle(&self, place: Option<WidgetPlace>) {
        if let Some(place) = place {
            if self.place_now().is_some_and(|now| (now.x, now.y) != (place.x, place.y)) {
                self.window.window().set_position(slint::PhysicalPosition::new(place.x, place.y));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_windows_default_to_separate_spots() {
        for scale in [1.0, 1.5, 2.0] {
            let monitor = Monitor { rect: Rect::new(0, 0, 3840, 2160), scale };
            let pet = default_place(Mode::Pet, monitor);
            let desk = default_place(Mode::Desk, monitor);
            assert_eq!((pet.w, pet.h), physical(BUBBLE, scale));
            assert_eq!((desk.w, desk.h), physical(DESK, scale));
            assert_eq!(pet.right(), desk.right());
            assert!(pet.bottom() < desk.y);
        }
    }
}
