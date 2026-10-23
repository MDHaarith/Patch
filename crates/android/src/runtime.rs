//! The UI thread. It owns the Slint platform, the [`App`] and the two
//! windows (the activity's and the floating overlay's), turns messages from
//! Java into Slint events, and paints frames into the Android surfaces.
//!
//! Nothing here touches JNI: Java is reached through [`Bridge`] and surfaces
//! through [`Surface`], so the whole thing runs on a desktop in tests.

use crate::buffer::{BackBuffer, Rect};
use crate::host::{AndroidHost, HostRequests};
use crate::ime::{ImeMode, ImeOp, ImeSnapshot};
use crate::input::{self, TouchAction};
use crate::platform::{Created, PatchPlatform};
use crate::window::SurfaceWindow;
use patch_ui::{Actions, App, MainWindow, OverlayWindow, Theme};
use slint::platform::{WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender};
use std::time::{Duration, Instant};

/// Which Java view a message is about. Numbered as in `PatchNative.java`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Main,
    Overlay,
}

impl Kind {
    pub fn from_java(kind: i32) -> Option<Kind> {
        match kind {
            0 => Some(Kind::Main),
            1 => Some(Kind::Overlay),
            _ => None,
        }
    }

    pub fn to_java(self) -> i32 {
        match self {
            Kind::Main => 0,
            Kind::Overlay => 1,
        }
    }
}

/// Something a finished frame can be shown on: an `ANativeWindow` on the
/// device, memory in tests.
pub trait Surface: Send {
    /// Puts `rect` of `frame` on screen. `frame` always holds the complete
    /// picture, so an implementation may copy more than `rect` when the
    /// platform asks it to.
    fn present(&mut self, frame: &BackBuffer, rect: Rect) -> Result<(), String>;
}

/// What the UI thread needs from the Java side. Every call returns promptly:
/// the UI thread never waits for the Java main thread (which may itself be
/// waiting for us in `surfaceDestroyed`).
pub trait Bridge {
    /// Show/hide the overlay service; `false` if the permission is missing
    /// (Java then opens the permission screen).
    fn set_overlay(&self, on: bool) -> bool;
    fn overlay_allowed(&self) -> bool;
    fn set_overlay_boot(&self, on: bool);
    fn overlay_expand(&self, expanded: bool);
    fn open_app(&self, task_id: Option<&str>);
    fn notify(&self, title: &str, body: &str);
    /// Replace the reminders Android's AlarmManager will deliver (a JSON array).
    fn schedule_reminders(&self, json: &str);
    fn set_clipboard(&self, text: &str);
    fn clipboard(&self) -> Option<String>;
    fn ime(&self, kind: Kind, mode: ImeMode, state: &ImeSnapshot, seq: i32);
    /// A frame requested with [`Msg::RedrawNeeded`] has been posted.
    fn frame_done(&self, kind: Kind);
    fn back_result(&self, consumed: bool);
    /// The app's look changed (0 matte, 1 glass, 2 light): system bar icons
    /// and the window background follow it.
    fn set_look(&self, look: i32);
    /// The home-screen widget's state changed (JSON, see
    /// [`patch_ui::Host::publish_snapshot`]); Java stores it and redraws the
    /// widgets.
    fn publish_snapshot(&self, json: &str);
    /// Opens the camera QR scanner; the result comes back as [`Msg::Scanned`].
    fn scan_qr(&self);
    fn battery_unrestricted(&self) -> bool;
    fn request_battery_unrestricted(&self);
}

pub enum Msg {
    /// A surface was created or changed size. Sizes in pixels.
    Surface {
        kind: Kind,
        surface: Box<dyn Surface>,
        width: u32,
        height: u32,
    },
    /// The surface is going away; reply once it's no longer used.
    SurfaceLost {
        kind: Kind,
        done: SyncSender<()>,
    },
    /// Java waits for a frame at the current size (`surfaceRedrawNeededAsync`).
    RedrawNeeded(Kind),
    /// The view is gone for good (activity destroyed, overlay removed).
    ViewGone(Kind),
    Touch {
        kind: Kind,
        action: TouchAction,
        x: f32,
        y: f32,
    },
    Key {
        kind: Kind,
        code: i32,
        unicode: u32,
        down: bool,
        seq: i32,
    },
    Ime {
        kind: Kind,
        op: ImeOp,
        seq: i32,
    },
    Back,
    /// System bar (and keyboard) insets in pixels.
    Insets {
        top: f32,
        bottom: f32,
    },
    Density(f32),
    OverlayAllowed(bool),
    /// The user hid the overlay from its notification.
    OverlayClosed,
    /// The activity paused: save, then reply.
    Pause(Option<SyncSender<()>>),
    OpenTask(String),
    /// The widget's "+": open the editor on a new task, once the main window
    /// exists if it doesn't yet.
    NewTask,
    /// Text of a scanned QR code.
    Scanned(String),
    Invoke(Box<dyn FnOnce() + Send>),
}

/// Longest the overlay waits for its new size after bubble <-> panel.
const RESIZE_HOLD: Duration = Duration::from_millis(400);
const FRAME: Duration = Duration::from_millis(16);
/// Upper bound on sleeping, so a missed wake-up can never stall the UI.
const MAX_SLEEP: Duration = Duration::from_secs(60);

enum Component {
    Main(MainWindow),
    Overlay(OverlayWindow),
}

impl Component {
    fn window(&self) -> &slint::Window {
        match self {
            Component::Main(c) => c.window(),
            Component::Overlay(c) => c.window(),
        }
    }
}

struct Slot {
    kind: Kind,
    component: Component,
    window: Rc<SurfaceWindow>,
    surface: Option<Box<dyn Surface>>,
    frame: BackBuffer,
    repaint_all: bool,
    present_all: bool,
    frame_waiters: u32,
    /// While set, frames are held back (the surface has the wrong size).
    hold: Option<(Instant, bool)>,
}

impl Slot {
    fn render(&mut self, now: Instant, bridge: &dyn Bridge) {
        if let Some((until, _)) = self.hold {
            if now < until {
                return;
            }
            log::warn!("{:?}: surface resize didn't arrive in time", self.kind);
            self.hold = None;
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let dirty = self
            .window
            .draw(&mut self.frame, std::mem::take(&mut self.repaint_all));
        let rect = if self.present_all {
            Some(Rect::full(self.frame.width(), self.frame.height()))
        } else {
            dirty
        };
        if let Some(rect) = rect.filter(|r| !r.is_empty()) {
            match surface.present(&self.frame, rect) {
                Ok(()) => self.present_all = false,
                Err(e) => {
                    log::warn!("{:?}: present failed: {e}", self.kind);
                    self.present_all = true;
                }
            }
        }
        self.ack_frames(bridge);
    }

    fn ack_frames(&mut self, bridge: &dyn Bridge) {
        for _ in 0..std::mem::take(&mut self.frame_waiters) {
            bridge.frame_done(self.kind);
        }
    }

    fn set_size(&mut self, width: u32, height: u32) {
        if self.frame.resize(width, height) {
            self.window
                .window()
                .set_size(PhysicalSize::new(width, height));
            self.repaint_all = true;
        }
    }

    fn set_scale(&mut self, scale: f32) {
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        self.window
            .window()
            .set_size(PhysicalSize::new(self.frame.width(), self.frame.height()));
        self.repaint_all = true;
    }
}

/// Whether a surface of this logical size shows the panel (vs. the bubble).
fn is_panel(width: u32, height: u32, scale: f32) -> bool {
    (width.max(height) as f32 / scale) > 120.0
}

pub struct Runtime {
    app: Rc<App>,
    bridge: Rc<dyn Bridge>,
    created: Created,
    requests: Rc<HostRequests>,
    scale: f32,
    insets: (f32, f32),
    main: Option<Slot>,
    overlay: Option<Slot>,
    last_frame: Instant,
    look: Option<i32>,
    /// A [`Msg::NewTask`] arrived before the main window.
    new_task: bool,
}

impl Runtime {
    /// Installs the Slint platform for this thread and loads the app. `wake`
    /// must feed the receiver later passed to [`Runtime::run`].
    pub fn new(
        data_dir: PathBuf,
        scale: f32,
        bridge: Rc<dyn Bridge>,
        wake: Sender<Msg>,
    ) -> Result<Runtime, String> {
        let created = Created::default();
        slint::platform::set_platform(Box::new(PatchPlatform::new(
            created.clone(),
            wake,
            bridge.clone(),
        )))
        .map_err(|e| format!("can't install the Slint platform: {e}"))?;
        let requests = Rc::new(HostRequests::default());
        let host = Rc::new(AndroidHost::new(bridge.clone(), requests.clone()));
        let app = App::new(data_dir, host);
        Ok(Runtime {
            app,
            bridge,
            created,
            requests,
            scale: if scale > 0.0 { scale } else { 1.0 },
            insets: (0.0, 0.0),
            main: None,
            overlay: None,
            last_frame: Instant::now(),
            look: None,
            new_task: false,
        })
    }

    pub fn app(&self) -> &Rc<App> {
        &self.app
    }

    pub fn main_window(&self) -> Option<MainWindow> {
        match &self.main {
            Some(Slot {
                component: Component::Main(c),
                ..
            }) => Some(c.clone_strong()),
            _ => None,
        }
    }

    pub fn overlay_window(&self) -> Option<OverlayWindow> {
        match &self.overlay {
            Some(Slot {
                component: Component::Overlay(c),
                ..
            }) => Some(c.clone_strong()),
            _ => None,
        }
    }

    /// Serves messages until the channel closes (never, in the app).
    pub fn run(mut self, rx: Receiver<Msg>) {
        while self.step(&rx, self.next_timeout()) {}
    }

    /// Waits up to `timeout` (forever if `None`) for messages, handles all
    /// that are queued, then advances timers and paints. Returns `false` once
    /// the channel is closed.
    pub fn step(&mut self, rx: &Receiver<Msg>, timeout: Option<Duration>) -> bool {
        let first = match timeout {
            Some(t) => match rx.recv_timeout(t) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return false,
            },
            None => match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => return false,
            },
        };
        slint::platform::update_timers_and_animations();
        // Everything that queued up while we slept or painted goes into the
        // same frame.
        for msg in first
            .into_iter()
            .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        {
            self.handle(msg);
        }
        self.frame();
        true
    }

    /// How long the loop may sleep: until the next Slint timer, the next
    /// animation frame, or the end of an overlay resize hold.
    pub fn next_timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut timeout = slint::platform::duration_until_next_timer_update()
            .unwrap_or(MAX_SLEEP)
            .min(MAX_SLEEP);
        for slot in self.slots() {
            if slot.surface.is_some() && slot.window.window().has_active_animations() {
                timeout = timeout.min((self.last_frame + FRAME).saturating_duration_since(now));
            }
            if let Some((until, _)) = slot.hold {
                timeout = timeout.min(until.saturating_duration_since(now));
            }
        }
        Some(timeout)
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Surface {
                kind,
                surface,
                width,
                height,
            } => self.bind(kind, surface, width, height),
            Msg::SurfaceLost { kind, done } => {
                let bridge = self.bridge.clone();
                if let Some(slot) = self.slot_mut(kind) {
                    slot.surface = None;
                    slot.ack_frames(&*bridge);
                }
                let _ = done.send(());
            }
            Msg::RedrawNeeded(kind) => match self.slot_mut(kind) {
                Some(slot) if slot.surface.is_some() => {
                    slot.frame_waiters += 1;
                    slot.present_all = true;
                }
                _ => self.bridge.frame_done(kind),
            },
            Msg::ViewGone(kind) => self.unbind(kind),
            Msg::Touch { kind, action, x, y } => self.touch(kind, action, x, y),
            Msg::Key {
                kind,
                code,
                unicode,
                down,
                seq,
            } => self.key(kind, code, unicode, down, seq),
            Msg::Ime { kind, op, seq } => {
                if let Some(slot) = self.slot_mut(kind) {
                    let events = slot.window.ime.borrow_mut().translate(op, seq);
                    dispatch(&slot.window, events);
                }
            }
            Msg::Back => {
                let consumed = self.app.back();
                self.bridge.back_result(consumed);
            }
            Msg::Insets { top, bottom } => {
                self.insets = (top, bottom);
                self.apply_insets();
            }
            Msg::Density(scale) => {
                if scale > 0.0 && scale != self.scale {
                    self.scale = scale;
                    for slot in [self.main.as_mut(), self.overlay.as_mut()]
                        .into_iter()
                        .flatten()
                    {
                        slot.set_scale(scale);
                    }
                    self.apply_insets();
                }
            }
            Msg::OverlayAllowed(allowed) => self.app.set_overlay_allowed(allowed),
            Msg::OverlayClosed => {
                // Goes through the same action as the settings switch so the
                // app's preference follows.
                match (&self.main, &self.overlay) {
                    (
                        Some(Slot {
                            component: Component::Main(c),
                            ..
                        }),
                        _,
                    ) => c.global::<Actions>().invoke_set_overlay(false),
                    (
                        _,
                        Some(Slot {
                            component: Component::Overlay(c),
                            ..
                        }),
                    ) => c.global::<Actions>().invoke_set_overlay(false),
                    _ => {
                        self.bridge.set_overlay(false);
                    }
                }
            }
            Msg::Pause(done) => {
                self.app.save_now();
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Msg::OpenTask(id) => self.app.open_task(&id),
            Msg::NewTask => {
                self.new_task = true;
                self.take_new_task();
            }
            Msg::Scanned(text) => self.app.pair_from_link(&text),
            Msg::Invoke(f) => f(),
        }
    }

    /// Advances animations and paints every window that has a surface.
    pub fn frame(&mut self) {
        slint::platform::update_timers_and_animations();
        let now = Instant::now();
        if let Some(expanded) = self.requests.overlay_expand.take() {
            if let Some(slot) = self.overlay.as_mut() {
                let current = is_panel(slot.frame.width(), slot.frame.height(), self.scale);
                if slot.surface.is_some() && current != expanded {
                    slot.hold = Some((now + RESIZE_HOLD, expanded));
                }
            }
        }
        let bridge = self.bridge.clone();
        for slot in [self.main.as_mut(), self.overlay.as_mut()]
            .into_iter()
            .flatten()
        {
            slot.render(now, &*bridge);
            // After painting: rendering itself can report a changed text.
            let pending = slot.window.ime.borrow_mut().take_pending();
            if let Some((mode, state, seq)) = pending {
                bridge.ime(slot.kind, mode, &state, seq);
            }
        }
        self.last_frame = now;
        self.sync_look();
    }

    fn sync_look(&mut self) {
        let Some(Slot {
            component: Component::Main(main),
            ..
        }) = &self.main
        else {
            return;
        };
        let theme = main.global::<Theme>();
        let look = if theme.get_light() {
            2
        } else if theme.get_glass() {
            1
        } else {
            0
        };
        if self.look != Some(look) {
            self.look = Some(look);
            self.bridge.set_look(look);
        }
    }

    /// Opens the editor on a new task if one was asked for and the main
    /// window is there to show it.
    fn take_new_task(&mut self) {
        if let Some(Slot {
            component: Component::Main(main),
            ..
        }) = &self.main
        {
            if std::mem::take(&mut self.new_task) {
                main.global::<Actions>().invoke_new_task("".into(), -2);
            }
        }
    }

    fn slots(&self) -> impl Iterator<Item = &Slot> {
        self.main.iter().chain(self.overlay.iter())
    }

    fn slot_mut(&mut self, kind: Kind) -> Option<&mut Slot> {
        match kind {
            Kind::Main => self.main.as_mut(),
            Kind::Overlay => self.overlay.as_mut(),
        }
    }

    fn bind(&mut self, kind: Kind, surface: Box<dyn Surface>, width: u32, height: u32) {
        if self.slot_mut(kind).is_none() {
            match self.create(kind) {
                Ok(slot) => match kind {
                    Kind::Main => self.main = Some(slot),
                    Kind::Overlay => self.overlay = Some(slot),
                },
                Err(e) => {
                    log::error!("can't create the {kind:?} window: {e}");
                    return;
                }
            }
            self.attach(kind, width, height);
        }
        let scale = self.scale;
        let Some(slot) = self.slot_mut(kind) else {
            return;
        };
        slot.surface = Some(surface);
        slot.present_all = true;
        slot.set_size(width, height);
        if let Some((_, expanded)) = slot.hold {
            if is_panel(width, height, scale) == expanded {
                slot.hold = None;
            }
        }
    }

    fn create(&mut self, kind: Kind) -> Result<Slot, String> {
        self.created.borrow_mut().clear();
        let component = match kind {
            Kind::Main => Component::Main(MainWindow::new().map_err(|e| e.to_string())?),
            Kind::Overlay => Component::Overlay(OverlayWindow::new().map_err(|e| e.to_string())?),
        };
        let window = self
            .created
            .borrow_mut()
            .pop()
            .ok_or("the component didn't create a window")?;
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: self.scale,
            });
        Ok(Slot {
            kind,
            component,
            window,
            surface: None,
            frame: BackBuffer::default(),
            repaint_all: true,
            present_all: true,
            frame_waiters: 0,
            hold: None,
        })
    }

    /// Sizes, attaches and shows a freshly created window.
    fn attach(&mut self, kind: Kind, width: u32, height: u32) {
        let app = self.app.clone();
        let Some(slot) = self.slot_mut(kind) else {
            return;
        };
        slot.set_size(width, height);
        match &slot.component {
            Component::Main(c) => app.attach_main(c),
            Component::Overlay(c) => app.attach_overlay(c),
        }
        let Some(slot) = self.slot_mut(kind) else {
            return;
        };
        if let Err(e) = slot.component.window().show() {
            log::error!("can't show the {kind:?} window: {e}");
        }
        match kind {
            Kind::Main => {
                self.apply_insets();
                self.take_new_task();
            }
            // Java sized the overlay from its own copy of the state; make sure
            // it matches the app's.
            Kind::Overlay => {
                let expanded = self.app.overlay_expanded();
                if is_panel(width, height, self.scale) != expanded {
                    self.requests.overlay_expand.set(Some(expanded));
                    self.bridge.overlay_expand(expanded);
                }
            }
        }
    }

    fn unbind(&mut self, kind: Kind) {
        let slot = match kind {
            Kind::Main => self.main.take(),
            Kind::Overlay => self.overlay.take(),
        };
        let Some(mut slot) = slot else { return };
        match kind {
            Kind::Main => self.app.detach_main(),
            Kind::Overlay => self.app.detach_overlay(),
        }
        let _ = slot.component.window().hide();
        slot.ack_frames(&*self.bridge);
        // Tell Java the keyboard has nothing to type into any more.
        self.bridge
            .ime(kind, ImeMode::Hide, &ImeSnapshot::default(), 0);
    }

    fn apply_insets(&self) {
        let (top, bottom) = self.insets;
        self.app
            .set_safe_area(top / self.scale, bottom / self.scale);
    }

    fn touch(&mut self, kind: Kind, action: TouchAction, x: f32, y: f32) {
        let scale = self.scale;
        let Some(slot) = self.slot_mut(kind) else {
            return;
        };
        if action == TouchAction::Down && slot.window.ime.borrow().has_preedit() {
            // A tap may move the cursor: settle the word being composed where
            // it is, and have Java restart the IME so it forgets it too.
            let events = slot.window.ime.borrow_mut().finish_composing();
            dispatch(&slot.window, events);
            slot.window.ime.borrow_mut().restart();
        }
        dispatch(
            &slot.window,
            input::touch_events(action, input::logical(x, y, scale)),
        );
    }

    fn key(&mut self, kind: Kind, code: i32, unicode: u32, down: bool, seq: i32) {
        let Some(slot) = self.slot_mut(kind) else {
            return;
        };
        let Some(text) = input::key_text(code, unicode) else {
            return;
        };
        let mut events = Vec::new();
        {
            let mut ime = slot.window.ime.borrow_mut();
            if down && text == slint::SharedString::from(slint::platform::Key::Return) {
                events = ime.finish_composing();
            }
            ime.applied(seq);
        }
        events.push(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
        dispatch(&slot.window, events);
    }
}

fn dispatch(window: &SurfaceWindow, events: Vec<WindowEvent>) {
    for event in events {
        window.window().dispatch_event(event);
    }
}
