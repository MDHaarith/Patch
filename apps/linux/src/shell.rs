//! The desktop shell around the shared UI: the main window, the always-above
//! pet, the desktop tile board, the tray icon and the embedded sync hub,
//! plus the platform services `patch_ui::Host` asks for.
//!
//! Patch is meant to stay on. Closing the main window hides it; the process
//! keeps running while anything still needs it (a tray icon to come back
//! through, the widget, the hub other devices sync with, or "Launch at login",
//! which asks for reminders around the clock). Otherwise closing quits.
//! Quitting is in the tray menu, the launcher's right-click menu
//! (`patch --quit`) and on SIGTERM.
//!
//! The main window has no system title bar: the shared UI draws its own
//! rounded frame, window buttons and drag area (`Host::custom_chrome`) and
//! asks the shell to move, resize, minimize, maximize or close the window
//! (`Host::window_action`). It can be pinned above other apps. The pet always
//! stays above visible apps and hides on the desktop; the desk tile board stays there.
//! These layers are kept up on X11 by `layers`: set when a window is shown,
//! checked again on focus changes and every few seconds, because window
//! managers drop states requested before a window is mapped.

use crate::layers::{self, Hints, Layer, Wm};
use crate::notify::Notifier;
use crate::screen::{self, Screen};
use crate::tray::{Toggles, Tray, TrayAction};
use crate::widget::{Mode, Widget};
use patch_linux::cli::Windowing;
use patch_linux::instance::Request;
use patch_linux::paths::Paths;
use patch_linux::state::{DesktopState, MainSize};
use patch_linux::{autostart, hosting};
use patch_sync::Hub;
use patch_ui::{Actions, App, Host, HostedHub, MainWindow, OverlayWindow, Store, WindowAction};
use slint::platform::WindowEvent as SlintEvent;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::window::{ResizeDirection, UserAttentionType, WindowLevel};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, LogicalSize, Timer, TimerMode};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// How often window layers and desktop pet visibility are checked again.
const LAYER_CHECK: Duration = Duration::from_secs(2);

thread_local! {
    static SHELL: RefCell<Weak<Shell>> = const { RefCell::new(Weak::new()) };
}

/// Runs `f` on the UI thread with the shell. Callable from any thread
/// (tray, signals, the instance socket, notification clicks). Returns false
/// when the event loop is gone.
pub fn post(f: impl FnOnce(&Rc<Shell>) + Send + 'static) -> bool {
    slint::invoke_from_event_loop(move || {
        if let Some(shell) = SHELL.with(|s| s.borrow().upgrade()) {
            f(&shell);
        }
    })
    .is_ok()
}

/// Defers `f` to the next event-loop turn. `Host` methods are called from
/// inside `App` callbacks; calling back into the `App` from there could find
/// its state mid-update.
fn later(f: impl FnOnce() + 'static) {
    Timer::single_shot(Duration::ZERO, f);
}

#[derive(Clone, Debug)]
pub struct Options {
    pub windowing: Windowing,
    /// `--screenshot`: no tray, no notifications.
    pub headless_extras: bool,
}

pub struct Shell {
    paths: Paths,
    opts: Options,
    me: Weak<Shell>,
    app: OnceCell<Weak<App>>,
    main: OnceCell<MainWindow>,
    main_sized: Cell<bool>,
    widget: RefCell<Option<Widget>>,
    desk: RefCell<Option<Widget>>,
    widget_wanted: Cell<bool>,
    widget_pending: Cell<bool>,
    /// The main window stays above other apps.
    pinned: Cell<bool>,
    /// EWMH connection for the layers; `None` until needed or off X11.
    wm: RefCell<Option<Wm>>,
    wm_tried: Cell<bool>,
    layer_timer: Timer,
    hub: RefCell<Option<Hub>>,
    tray: RefCell<Option<Tray>>,
    notifier: Option<Notifier>,
    clipboard: RefCell<Option<arboard::Clipboard>>,
    state: RefCell<DesktopState>,
    state_timer: Timer,
    wayland: Cell<bool>,
    quitting: Cell<bool>,
}

impl Shell {
    pub fn new(paths: Paths, opts: Options) -> Rc<Shell> {
        let notifier = (!opts.headless_extras).then(|| {
            Notifier::new(crate::icon::notification_icon(&paths.cache), || {
                post(|s| s.show_main());
            })
        });
        let state = DesktopState::load(&paths.state_file());
        let shell = Rc::new_cyclic(|me| Shell {
            paths,
            opts,
            me: me.clone(),
            app: OnceCell::new(),
            main: OnceCell::new(),
            main_sized: Cell::new(false),
            widget: RefCell::new(None),
            desk: RefCell::new(None),
            widget_wanted: Cell::new(false),
            widget_pending: Cell::new(false),
            pinned: Cell::new(false),
            wm: RefCell::new(None),
            wm_tried: Cell::new(false),
            layer_timer: Timer::default(),
            hub: RefCell::new(None),
            tray: RefCell::new(None),
            notifier,
            clipboard: RefCell::new(None),
            state: RefCell::new(state),
            state_timer: Timer::default(),
            wayland: Cell::new(std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())),
            quitting: Cell::new(false),
        });
        SHELL.with(|s| *s.borrow_mut() = Rc::downgrade(&shell));
        shell
    }

    fn app(&self) -> Option<Rc<App>> {
        self.app.get().and_then(Weak::upgrade)
    }

    pub fn main(&self) -> Option<&MainWindow> {
        self.main.get()
    }

    /// The look (theme index) in the preferences.
    pub fn look(&self) -> i32 {
        self.app().map_or(0, |a| a.prefs().look)
    }

    pub fn widget_window(&self) -> Option<OverlayWindow> {
        self.widget.borrow().as_ref().map(|w| w.window().clone_strong())
    }

    /// Creates the main window (hidden) and attaches everything to `app`.
    pub fn attach(&self, app: &Rc<App>) -> Result<(), slint::PlatformError> {
        let _ = self.app.set(Rc::downgrade(app));
        let main = MainWindow::new()?;
        let me = self.me.clone();
        main.window().on_close_requested(move || {
            if let Some(shell) = me.upgrade() {
                shell.main_closed();
            }
            CloseRequestResponse::HideWindow
        });
        // Focus moving to another app is when a window manager may drop "above".
        let me = self.me.clone();
        // Resized also follows maximizing from the window manager (keyboard
        // shortcut, window menu) into the custom title bar's button.
        main.window().on_winit_window_event(move |_, event| {
            match event {
                WindowEvent::Focused(_) => {
                    if let Some(s) = me.upgrade() {
                        s.layers_soon();
                    }
                }
                WindowEvent::Resized(_) => {
                    let me = me.clone();
                    later(move || {
                        if let Some(main) = me.upgrade().as_ref().and_then(|s| s.main.get()) {
                            let maximized = main.window().is_maximized();
                            let store = main.global::<Store>();
                            if store.get_maximized() != maximized {
                                store.set_maximized(maximized);
                            }
                        }
                    });
                }
                _ => {}
            }
            EventResult::Propagate
        });
        app.attach_main(&main);
        let _ = self.main.set(main);

        let prefs = app.prefs();
        // The App applies these too; reading them here covers windows created
        // before it does.
        self.pinned.set(prefs.pinned);
        self.reconcile_autostart(prefs.autostart);
        let me = self.me.clone();
        self.layer_timer.start(TimerMode::Repeated, LAYER_CHECK, move || {
            if let Some(s) = me.upgrade() {
                s.apply_layers(false);
            }
        });
        // The App only starts the hub by itself; the widget is ours to restore.
        if prefs.widget {
            self.set_widget(true);
        }
        Ok(())
    }

    fn toggles(&self) -> Toggles {
        Toggles { widget: self.widget_wanted.get(), pinned: self.pinned.get() }
    }

    fn update_tray(&self) {
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_toggles(self.toggles());
        }
    }

    pub fn start_tray(&self) {
        match Tray::spawn(self.toggles(), |action| {
            post(move |s| s.on_tray(action));
        }) {
            Ok(tray) => {
                tray.set_hub(self.hub.borrow().as_ref().map(Hub::port));
                *self.tray.borrow_mut() = Some(tray);
            }
            Err(e) => eprintln!("patch: no tray icon: {e}"),
        }
    }

    fn tray_live(&self) -> bool {
        self.tray.borrow().as_ref().is_some_and(Tray::is_live)
    }

    fn on_tray(&self, action: TrayAction) {
        match action {
            TrayAction::Open => self.show_main(),
            // Through the App, so the preference and the Settings toggle follow.
            TrayAction::SetWidget(on) => match self.main.get() {
                Some(main) => main.global::<Actions>().invoke_set_widget(on),
                None => self.set_widget(on),
            },
            TrayAction::SetPinned(on) => match self.main.get() {
                Some(main) => main.global::<Actions>().invoke_set_pinned(on),
                None => self.set_pinned(on),
            },
            TrayAction::Quit => self.quit(),
        }
    }

    pub fn on_request(&self, req: Request) {
        match req {
            Request::Show => self.show_main(),
            Request::Quit => self.quit(),
            Request::Ping => {}
        }
    }

    // ------------------------------------------------------------- main window

    pub fn show_main(&self) {
        let Some(main) = self.main.get() else { return };
        let window = main.window();
        if window.is_visible() {
            self.raise(window);
            return;
        }
        if !self.main_sized.replace(true) {
            if let Some(m) = self.state.borrow().main {
                window.set_size(LogicalSize::new(m.width.max(360.0), m.height.max(520.0)));
                if m.maximized {
                    window.set_maximized(true);
                }
            }
        }
        if let Err(e) = main.show() {
            eprintln!("patch: can't show the window: {e}");
        }
        self.layers_after_show();
    }

    pub fn show_main_sized(&self, size: LogicalSize) {
        if let Some(main) = self.main.get() {
            self.main_sized.set(true);
            main.window().set_size(size);
            if let Err(e) = main.show() {
                eprintln!("patch: can't show the window: {e}");
            }
            self.layers_after_show();
        }
    }

    fn raise(&self, window: &slint::Window) {
        // On X11 un-minimizing also activates the window.
        window.set_minimized(false);
        window.with_winit_window(|w| {
            w.focus_window();
            // Wayland lets no client raise itself; this is the sanctioned
            // way to ask for the user's attention instead.
            if self.wayland.get() && !w.has_focus() {
                w.request_user_attention(Some(UserAttentionType::Informational));
            }
        });
    }

    fn remember_main_size(&self) {
        let Some(main) = self.main.get() else { return };
        let window = main.window();
        if !window.is_visible() {
            return;
        }
        let size = window.size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let maximized = window.is_maximized();
        let mut st = self.state.borrow_mut();
        let mut m = st.main.unwrap_or(MainSize { width: 1320.0, height: 840.0, maximized });
        // A maximized window's size isn't the one to come back to.
        if !maximized && !window.is_fullscreen() {
            let logical = size.to_logical(window.scale_factor());
            m.width = logical.width;
            m.height = logical.height;
        }
        m.maximized = maximized;
        st.main = Some(m);
        drop(st);
        self.save_state_soon();
    }

    fn keeps_running(&self) -> bool {
        self.tray_live()
            || self.widget_wanted.get()
            || self.hub.borrow().is_some()
            || self.app().is_some_and(|a| a.prefs().autostart)
    }

    fn main_closed(&self) {
        self.remember_main_size();
        if let Some(app) = self.app() {
            app.save_now();
        }
        if self.quitting.get() {
            return;
        }
        if self.keeps_running() {
            self.background_hint();
        } else {
            let me = self.me.clone();
            later(move || {
                if let Some(s) = me.upgrade() {
                    s.quit();
                }
            });
        }
    }

    /// Explains once where Patch went after its window closed.
    fn background_hint(&self) {
        if self.state.borrow().background_hint {
            return;
        }
        let mut reasons = Vec::new();
        if self.hub.borrow().is_some() {
            reasons.push("your other devices keep syncing");
        }
        reasons.push("reminders still arrive");
        let body = if self.tray_live() {
            format!("Patch is still running so {}. Open it or quit from the tray icon.", reasons.join(" and "))
        } else {
            format!(
                "Patch is still running so {}. Open it again from your apps; to quit, right-click Patch \
                 in the dock or run “patch --quit”.",
                reasons.join(" and ")
            )
        };
        self.notify_user("Patch keeps running", &body);
        self.state.borrow_mut().background_hint = true;
        self.save_state_soon();
    }

    // ------------------------------------------------------------------ widget

    fn sync_widget(&self) {
        let want = self.widget_wanted.get();
        let have = self.widget.borrow().is_some();
        if want && !have && !self.widget_pending.replace(true) {
            let me = self.me.clone();
            screen::query(move |screen| {
                let Some(s) = me.upgrade() else { return };
                let shell = s.clone();
                if let Err(e) = slint::spawn_local(async move {
                    if shell.widget_wanted.get() && shell.widget.borrow().is_none() {
                        shell.create_widget(&screen).await;
                    }
                    shell.widget_pending.set(false);
                }) {
                    s.widget_pending.set(false);
                    eprintln!("patch: can't start the widget: {e}");
                }
            });
        } else if !want && have {
            self.destroy_widget();
        }
    }

    async fn create_widget(&self, screen: &Screen) {
        let Some(app) = self.app() else { return };
        self.wayland.set(screen.wayland);
        for (mode, slot) in [(Mode::Desk, &self.desk), (Mode::Pet, &self.widget)] {
            let window = match OverlayWindow::new() {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("patch: can't create the {mode:?} widget: {e}");
                    self.destroy_widget();
                    return;
                }
            };
            // Alt+F4 on either window turns both off, like the Settings toggle.
            let me = self.me.clone();
            window.window().on_close_requested(move || {
                if let Some(s) = me.upgrade() {
                    s.on_tray(TrayAction::SetWidget(false));
                }
                CloseRequestResponse::HideWindow
            });
            let me = self.me.clone();
            let me2 = self.me.clone();
            let widget = Widget::new(
                window,
                mode,
                move |x, y| {
                    if let Some(s) = me.upgrade() {
                        s.widget_moved(mode, x, y);
                    }
                },
                move |_| {
                    if let Some(s) = me2.upgrade() {
                        s.layers_soon();
                    }
                },
            );
            let saved = match mode {
                Mode::Pet => {
                    app.attach_overlay(widget.window());
                    self.state.borrow().widget
                }
                Mode::Desk => {
                    app.attach_desk(widget.window());
                    self.state.borrow().desk
                }
            };
            let place = widget.place(screen, saved);
            if let Err(e) = widget.window().window().winit_window().await {
                eprintln!("patch: can't create the native {mode:?} widget: {e}");
                self.destroy_widget();
                return;
            }
            if !self.widget_wanted.get() {
                self.destroy_widget();
                return;
            }
            let hints = match mode {
                Mode::Pet => self.widget_hints(),
                Mode::Desk => self.desk_hints(),
            };
            self.apply_layer(widget.window().window(), hints, true);
            let show = mode == Mode::Desk || self.pet_visible(widget.window().window());
            if let Err(e) = if show { widget.show() } else { Ok(()) } {
                eprintln!("patch: can't show the {mode:?} widget: {e}");
                widget.hide();
                self.destroy_widget();
                return;
            }
            *slot.borrow_mut() = Some(widget);
            if let Some(place) = place {
                match mode {
                    Mode::Pet => self.state.borrow_mut().widget = Some(place),
                    Mode::Desk => self.state.borrow_mut().desk = Some(place),
                }
                self.save_state_soon();
            }
            let me = self.me.clone();
            Timer::single_shot(Duration::from_millis(300), move || {
                if let Some(s) = me.upgrade() {
                    let slot = match mode {
                        Mode::Pet => &s.widget,
                        Mode::Desk => &s.desk,
                    };
                    if let Some(w) = slot.borrow().as_ref() {
                        w.settle(place);
                    }
                }
            });
        }
        self.layers_after_show();
        if screen.wayland {
            self.wayland_layer_hint();
        }
    }

    fn destroy_widget(&self) {
        for (mode, slot) in [(Mode::Pet, &self.widget), (Mode::Desk, &self.desk)] {
            if let Some(widget) = slot.borrow_mut().take() {
                if let Some(place) = widget.place_now() {
                    match mode {
                        Mode::Pet => self.state.borrow_mut().widget = Some(place),
                        Mode::Desk => self.state.borrow_mut().desk = Some(place),
                    }
                    self.save_state_soon();
                }
                widget.hide();
            }
        }
        if let Some(app) = self.app() {
            app.detach_overlay();
            app.detach_desk();
        }
    }

    fn widget_moved(&self, mode: Mode, x: i32, y: i32) {
        let slot = match mode {
            Mode::Pet => &self.widget,
            Mode::Desk => &self.desk,
        };
        // Placement and settling may cause a move while the slot is borrowed.
        if !slot.try_borrow().is_ok_and(|w| w.is_some()) {
            return;
        }
        let place = Some(patch_linux::state::WidgetPlace { x, y, expanded: false });
        match mode {
            Mode::Pet => self.state.borrow_mut().widget = place,
            Mode::Desk => self.state.borrow_mut().desk = place,
        }
        self.save_state_soon();
    }

    // ------------------------------------------------------------------ layers

    fn main_hints(&self) -> Hints {
        let layer = if self.pinned.get() { Layer::Above } else { Layer::Normal };
        Hints { layer, sticky: false, skip_taskbar: false }
    }

    fn widget_hints(&self) -> Hints {
        Hints { layer: Layer::Above, sticky: true, skip_taskbar: true }
    }

    fn desk_hints(&self) -> Hints {
        Hints { layer: Layer::Desktop, sticky: true, skip_taskbar: true }
    }

    fn pet_visible(&self, window: &slint::Window) -> bool {
        let own: Vec<_> = self
            .main
            .get()
            .map(|m| m.window())
            .into_iter()
            .chain(std::iter::once(window))
            .filter_map(layers::x11_id)
            .chain(self.desk.borrow().as_ref().and_then(|d| layers::x11_id(d.window().window())))
            .collect();
        self.wm.borrow().as_ref().is_none_or(|wm| wm.pet_visible(&own).unwrap_or(true))
    }

    /// Sets `hints` before or after mapping. Without `strict`, only adds what is
    /// missing (see `Wm::apply`).
    fn apply_layer(&self, window: &slint::Window, hints: Hints, strict: bool) {
        if let Some(id) = layers::x11_id(window) {
            if !self.wm_tried.replace(true) {
                match Wm::connect() {
                    Ok(wm) => *self.wm.borrow_mut() = Some(wm),
                    Err(e) => eprintln!("patch: can't talk to the window manager: {e}"),
                }
            }
            if let Some(wm) = self.wm.borrow().as_ref() {
                if let Err(e) = wm.apply(id, hints, strict) {
                    eprintln!("patch: can't set the window layer: {e}");
                }
                return;
            }
        }
        // Wayland (where this asks the compositor in vain) or no X connection.
        if strict {
            let level = match hints.layer {
                Layer::Normal => WindowLevel::Normal,
                Layer::Above => WindowLevel::AlwaysOnTop,
                Layer::Desktop => WindowLevel::AlwaysOnBottom,
            };
            window.with_winit_window(|w| w.set_window_level(level));
        }
    }

    fn apply_layers(&self, strict: bool) {
        if let Some(main) = self.main.get() {
            // An unpinned window needs nothing kept up.
            if strict || self.pinned.get() {
                self.apply_layer(main.window(), self.main_hints(), strict);
            }
        }
        if let Some(w) = self.desk.borrow().as_ref() {
            self.apply_layer(w.window().window(), self.desk_hints(), strict);
        }
        if let Some(w) = self.widget_window() {
            let visible = self.pet_visible(w.window());
            let changed = visible != w.window().is_visible();
            if changed {
                let result = if visible { w.show() } else { w.hide() };
                if let Err(e) = result {
                    eprintln!("patch: can't change pet visibility: {e}");
                }
            }
            if visible {
                self.apply_layer(w.window(), self.widget_hints(), strict || changed);
            }
        }
    }

    /// Checks the layers once the current event is handled.
    fn layers_soon(&self) {
        let me = self.me.clone();
        later(move || {
            if let Some(s) = me.upgrade() {
                s.apply_layers(false);
            }
        });
    }

    /// Sets the layers on a window that was just shown, and again once the
    /// window manager has mapped it.
    fn layers_after_show(&self) {
        self.apply_layers(true);
        for ms in [150, 600] {
            let me = self.me.clone();
            Timer::single_shot(Duration::from_millis(ms), move || {
                if let Some(s) = me.upgrade() {
                    s.apply_layers(true);
                }
            });
        }
    }

    fn wayland_layer_hint(&self) {
        if self.state.borrow().wayland_widget_hint {
            return;
        }
        let body = match self.opts.windowing {
            Windowing::Auto => {
                "Wayland doesn't let apps keep a window above or below others. XWayland isn't available \
                 in this session. Enable XWayland and restart Patch to keep the pet above apps and \
                 the desk widget on the desktop."
            }
            _ => {
                "Wayland doesn't let apps keep a window above or below others. Use your desktop's \
                 “Keep Above/Below Others” window options, or start Patch with --x11."
            }
        };
        self.notify_user("Window layers", body);
        self.state.borrow_mut().wayland_widget_hint = true;
        self.save_state_soon();
    }

    // ------------------------------------------------------------- lifecycle

    fn notify_user(&self, summary: &str, body: &str) {
        match &self.notifier {
            Some(n) => n.send(summary, body),
            None => eprintln!("patch: {summary}: {body}"),
        }
    }

    fn save_state_soon(&self) {
        let me = self.me.clone();
        self.state_timer.start(TimerMode::SingleShot, Duration::from_millis(600), move || {
            if let Some(s) = me.upgrade() {
                s.state.borrow().save(&s.paths.state_file());
            }
        });
    }

    fn reconcile_autostart(&self, pref: bool) {
        let file = self.paths.autostart_file();
        if autostart::is_enabled(&file) {
            if let Ok(exe) = autostart::current_exe() {
                let _ = autostart::refresh(&file, &exe);
            }
        } else if pref {
            // Removed through the desktop's startup settings: follow along.
            if let Some(main) = self.main.get() {
                main.global::<Actions>().invoke_set_autostart(false);
            }
        }
    }

    /// Saves everything and leaves the event loop.
    pub fn quit(&self) {
        if self.quitting.replace(true) {
            return;
        }
        self.remember_main_size();
        for (mode, slot) in [(Mode::Pet, &self.widget), (Mode::Desk, &self.desk)] {
            if let Some(place) = slot.borrow().as_ref().and_then(Widget::place_now) {
                match mode {
                    Mode::Pet => self.state.borrow_mut().widget = Some(place),
                    Mode::Desk => self.state.borrow_mut().desk = Some(place),
                }
            }
        }
        if let Some(app) = self.app() {
            app.save_now();
        }
        let _ = slint::quit_event_loop();
    }

    /// Cleanup after the event loop returned.
    pub fn shutdown(&self) {
        self.quitting.set(true);
        self.destroy_widget();
        if let Some(app) = self.app() {
            app.save_now();
            app.detach_main();
        }
        if let Some(main) = self.main.get() {
            let _ = main.hide();
        }
        self.state_timer.stop();
        self.state.borrow().save(&self.paths.state_file());
        if let Some(hub) = self.hub.borrow_mut().take() {
            hub.stop();
        }
        if let Some(tray) = self.tray.borrow_mut().take() {
            tray.shutdown();
        }
    }
}

impl Host for Shell {
    fn can_host_hub(&self) -> bool {
        true
    }

    fn set_hosting(&self, on: bool) -> Result<Option<HostedHub>, String> {
        if !on {
            if let Some(hub) = self.hub.borrow_mut().take() {
                hub.stop();
            }
            if let Some(tray) = self.tray.borrow().as_ref() {
                tray.set_hub(None);
            }
            return Ok(None);
        }
        let mut slot = self.hub.borrow_mut();
        if slot.is_none() {
            let dir = self.paths.hub_dir();
            *slot = Some(hosting::start(&dir, hosting::remembered_port(&dir))?);
        }
        let hub = slot.as_ref().expect("hub was just started");
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_hub(Some(hub.port()));
        }
        Ok(Some(HostedHub {
            local_url: format!("http://127.0.0.1:{}", hub.port()),
            urls: hub.urls(),
            token: hub.token().to_string(),
        }))
    }

    fn set_autostart(&self, on: bool) -> Result<(), String> {
        let file = self.paths.autostart_file();
        let result = if on {
            autostart::current_exe().and_then(|exe| autostart::enable(&file, &exe))
        } else {
            autostart::disable(&file)
        };
        result.map_err(|e| format!("Couldn't update {}: {e}", file.display()))
    }

    fn set_widget(&self, on: bool) {
        self.widget_wanted.set(on);
        self.update_tray();
        let me = self.me.clone();
        later(move || {
            if let Some(s) = me.upgrade() {
                s.sync_widget();
            }
        });
    }

    fn custom_chrome(&self) -> bool {
        true
    }

    fn window_action(&self, action: WindowAction) {
        let Some(main) = self.main.get() else { return };
        let window = main.window();
        match action {
            WindowAction::Drag | WindowAction::Resize(_) => {
                let result = window.with_winit_window(|w| match action {
                    WindowAction::Resize(edge) => w.drag_resize_window(resize_direction(edge)),
                    _ => w.drag_window(),
                });
                if let Some(Err(e)) = result {
                    eprintln!("patch: can't move the window: {e}");
                }
                // The window manager takes the pointer, release included, for
                // the move: end the press so the title bar doesn't stay grabbed.
                let weak = main.as_weak();
                later(move || {
                    if let Some(m) = weak.upgrade() {
                        m.window().dispatch_event(SlintEvent::PointerExited);
                    }
                });
            }
            // Like the window manager's close button used to: hide, and keep
            // running or quit by the same rules.
            WindowAction::Close => {
                let me = self.me.clone();
                later(move || {
                    if let Some(s) = me.upgrade() {
                        s.main_closed();
                        if let Some(m) = s.main.get() {
                            let _ = m.hide();
                        }
                    }
                });
            }
            WindowAction::Minimize => window.set_minimized(true),
            WindowAction::ToggleMaximize => {
                let maximized = window.is_maximized();
                window.set_maximized(!maximized);
            }
        }
    }

    fn set_pinned(&self, on: bool) {
        self.pinned.set(on);
        self.update_tray();
        if on && self.wayland.get() && self.main.get().is_some_and(|m| m.window().is_visible()) {
            self.wayland_layer_hint();
        }
        let me = self.me.clone();
        later(move || {
            if let Some(s) = me.upgrade() {
                if let Some(main) = s.main.get() {
                    s.apply_layer(main.window(), s.main_hints(), true);
                }
            }
        });
    }

    fn overlay_expand(&self, expanded: bool) {
        let place = self.widget.borrow().as_ref().and_then(|w| w.resize(expanded));
        if let Some(place) = place {
            self.state.borrow_mut().widget = Some(place);
            self.save_state_soon();
        }
    }

    fn open_app(&self, _task_id: Option<&str>) {
        // The App opens the task itself after this returns.
        self.show_main();
    }

    fn notify(&self, title: &str, body: &str) {
        self.notify_user(title, body);
    }

    fn copy_text(&self, text: &str) {
        let mut clipboard = self.clipboard.borrow_mut();
        if clipboard.is_none() {
            // Kept alive: on X11 the owner of a selection serves it.
            *clipboard = arboard::Clipboard::new().map_err(|e| eprintln!("patch: no clipboard: {e}")).ok();
        }
        if let Some(c) = clipboard.as_mut() {
            if let Err(e) = c.set_text(text) {
                eprintln!("patch: can't copy: {e}");
            }
        }
    }
}

fn resize_direction(edge: u8) -> ResizeDirection {
    match edge {
        0 => ResizeDirection::North,
        1 => ResizeDirection::NorthEast,
        2 => ResizeDirection::East,
        3 => ResizeDirection::SouthEast,
        4 => ResizeDirection::South,
        5 => ResizeDirection::SouthWest,
        6 => ResizeDirection::West,
        _ => ResizeDirection::NorthWest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_and_desk_have_separate_layers() {
        let shell = Shell::new(
            Paths::from_env(Some("/nonexistent/patch-layer-test".into())),
            Options { windowing: Windowing::Auto, headless_extras: true },
        );
        assert_eq!(shell.widget_hints(), Hints { layer: Layer::Above, sticky: true, skip_taskbar: true });
        assert_eq!(shell.desk_hints(), Hints { layer: Layer::Desktop, sticky: true, skip_taskbar: true });
    }
}
