//! Monitor layout and windowing system, read from winit's active event loop
//! so a window can be placed before it is first shown.

use patch_linux::geometry::Rect;
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::winit::monitor::MonitorHandle;
use slint::winit_030::winit::platform::wayland::ActiveEventLoopExtWayland;
use std::cell::RefCell;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub rect: Rect,
    pub scale: f64,
}

impl Monitor {
    pub fn from_handle(m: &MonitorHandle) -> Monitor {
        let (p, s) = (m.position(), m.size());
        Monitor { rect: Rect::new(p.x, p.y, s.width as i32, s.height as i32), scale: m.scale_factor() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Screen {
    /// Wayland gives clients no window positions and no "always on top".
    pub wayland: bool,
    pub monitors: Vec<Monitor>,
    pub primary: Option<Monitor>,
}

impl Screen {
    fn from_event_loop(el: &ActiveEventLoop) -> Screen {
        let monitors: Vec<Monitor> = el.available_monitors().map(|m| Monitor::from_handle(&m)).collect();
        // Wayland has no primary monitor; take the first one.
        let primary = el.primary_monitor().map(|m| Monitor::from_handle(&m)).or_else(|| monitors.first().copied());
        Screen { wayland: el.is_wayland(), monitors, primary }
    }

    pub fn monitor_of(&self, r: &Rect) -> Option<Monitor> {
        let rects: Vec<Rect> = self.monitors.iter().map(|m| m.rect).collect();
        let best = patch_linux::geometry::monitor_of(r, &rects)?;
        self.monitors.iter().copied().find(|m| m.rect == best)
    }
}

type Pending = Box<dyn FnOnce(Screen)>;

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

/// Calls `then` with the screen layout on the UI thread. Asynchronous,
/// because winit only exposes monitors from inside its event loop.
pub fn query(then: impl FnOnce(Screen) + 'static) {
    PENDING.with(|p| p.borrow_mut().push(Box::new(then)));
    let sent = slint::winit_030::invoke_from_active_event_loop(|el| {
        // Runs on the UI thread, the one that queued the callbacks.
        let screen = Screen::from_event_loop(el);
        for f in PENDING.with(|p| p.take()) {
            f(screen.clone());
        }
    });
    if sent.is_err() {
        for f in PENDING.with(|p| p.take()) {
            f(Screen::default());
        }
    }
}
