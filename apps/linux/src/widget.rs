//! A pet that expands into the task panel, above apps, and a separate
//! 352×352 tile board on the desktop. The shell sets their layers and visibility.
//!
//! Both windows can be dragged anywhere. A press is held back from Slint
//! until it is clear whether it is a click (press and release are delivered
//! together, so tapping items still works) or a drag (`drag_window()` hands
//! the pointer to the window manager). This avoids leaving Slint waiting for
//! a release the window manager swallows during the move.

use crate::screen::{Monitor, Screen};
use patch_linux::geometry::{default_rect, resize_anchored, Rect};
use patch_linux::state::WidgetPlace;
use patch_ui::{OverlayWindow, Store};
use slint::platform::{PointerEventButton, WindowEvent as SlintEvent};
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::Cell;
use std::rc::Rc;

pub const BUBBLE: (f32, f32) = (60.0, 60.0);
pub const PANEL: (f32, f32) = (340.0, 500.0);
const HEADER_HEIGHT: f64 = 52.0;
const HEADER_BUTTONS: f64 = 84.0;
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
    /// Logical offset inside the panel, so dragging between screen scales keeps the anchor.
    bubble_offset: Cell<Option<(f32, f32)>>,
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
                    if let Some(at) = pointer.at.get() {
                        let expanded = mode == Mode::Pet
                            && weak.upgrade().is_some_and(|o| o.global::<Store>().get_overlay_expanded());
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
                WindowEvent::Focused(f) => on_focus(*f),
                _ => {}
            }
            EventResult::Propagate
        });
        Widget { window, mode, bubble_offset: Cell::new(None) }
    }

    pub fn window(&self) -> &OverlayWindow {
        &self.window
    }

    pub fn expanded(&self) -> bool {
        self.mode == Mode::Pet && self.window.global::<Store>().get_overlay_expanded()
    }

    /// Sizes and positions the window before it is first shown. On X11 it
    /// goes back to `saved` (re-anchored if it was saved in the other mode),
    /// or to the top-right corner the first time. Wayland places it itself.
    pub fn place(&self, screen: &Screen, saved: Option<WidgetPlace>) -> Option<WidgetPlace> {
        let expanded = self.expanded();
        let (w, h) = self.size_for(expanded);
        self.window.window().set_size(LogicalSize::new(w, h));
        if screen.wayland {
            return None;
        }
        let rect = saved
            .and_then(|s| {
                // Off-screen (monitor unplugged since): fall back to the default spot.
                let (lw, lh) = physical(self.size_for(s.expanded), 1.0);
                let monitor = screen.monitor_of(&Rect::new(s.x, s.y, lw, lh))?;
                let (sw, sh) = physical(self.size_for(s.expanded), monitor.scale);
                let old = Rect::new(s.x, s.y, sw, sh);
                let (nw, nh) = physical((w, h), monitor.scale);
                let rect = if s.expanded == expanded {
                    Rect::new(old.x, old.y, nw, nh).clamped_to(&monitor.rect)
                } else {
                    resize_anchored(old, nw, nh, &monitor.rect, None)
                };
                if expanded && !s.expanded {
                    self.bubble_offset.set(Some((
                        (old.x - rect.x) as f32 / monitor.scale as f32,
                        (old.y - rect.y) as f32 / monitor.scale as f32,
                    )));
                }
                Some(rect)
            })
            .or_else(|| {
                let m = screen.primary?;
                let (pw, ph) = physical((w, h), m.scale);
                Some(if self.mode == Mode::Desk {
                    default_place(self.mode, m)
                } else {
                    default_rect(pw, ph, &m.rect, m.scale as f32)
                })
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
        self.window.window().with_winit_window(|ww| ww.outer_position().ok()).flatten().map(|p| WidgetPlace {
            x: p.x,
            y: p.y,
            expanded: self.expanded(),
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
    fn size_for(&self, expanded: bool) -> (f32, f32) {
        if self.mode == Mode::Pet && expanded {
            PANEL
        } else {
            self.mode.size()
        }
    }

    /// Switches between bubble and panel size. On X11 the corner nearest the
    /// screen edge stays where it is; collapse restores the saved bubble offset.
    pub fn resize(&self, expanded: bool) -> Option<WidgetPlace> {
        let (w, h) = self.size_for(expanded);
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
                if (old.w, old.h) == (nw, nh) {
                    return Some(old.clamped_to(&monitor.rect));
                }
                let offset = if expanded {
                    None
                } else {
                    self.bubble_offset.get().map(|offset| physical(offset, ww.scale_factor()))
                };
                let rect = resize_anchored(old, nw, nh, &monitor.rect, offset);
                if expanded {
                    self.bubble_offset.set(Some((
                        (old.x - rect.x) as f32 / ww.scale_factor() as f32,
                        (old.y - rect.y) as f32 / ww.scale_factor() as f32,
                    )));
                }
                Some(rect)
            })
            .flatten();
        self.window.window().set_size(LogicalSize::new(w, h));
        let rect = target?;
        self.window.window().set_position(slint::PhysicalPosition::new(rect.x, rect.y));
        Some(WidgetPlace { x: rect.x, y: rect.y, expanded })
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
