//! The desktop shell around the shared UI: the main window, the floating
//! widget, the tray icon and the embedded sync hub, plus the platform
//! services `patch_ui::Host` asks for.
//!
//! Patch is meant to stay on. Closing the main window hides it; the process
//! keeps running while anything still needs it (a tray icon to come back
//! through, the widget, the hub other devices sync with, or "Launch at login",
//! which asks for reminders around the clock). Otherwise closing quits.
//! Quitting is in the tray menu, the launcher's right-click menu
//! (`patch --quit`) and on SIGTERM.

use crate::notify::Notifier;
use crate::screen::{self, Screen};
use crate::tray::{Tray, TrayAction};
use crate::widget::Widget;
use patch_linux::cli::Windowing;
use patch_linux::instance::Request;
use patch_linux::paths::Paths;
use patch_linux::state::{DesktopState, MainSize};
use patch_linux::{autostart, hosting};
use patch_sync::Hub;
use patch_ui::{Actions, App, Host, HostedHub, MainWindow, OverlayWindow};
use slint::winit_030::winit::window::UserAttentionType;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle, LogicalSize, Timer, TimerMode};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

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
    widget_wanted: Cell<bool>,
    widget_pending: Cell<bool>,
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
            widget_wanted: Cell::new(false),
            widget_pending: Cell::new(false),
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
        app.attach_main(&main);
        let _ = self.main.set(main);

        let prefs = app.prefs();
        self.reconcile_autostart(prefs.autostart);
        // The App only starts the hub by itself; the widget is ours to restore.
        if prefs.widget {
            self.set_widget(true);
        }
        Ok(())
    }

    pub fn start_tray(&self) {
        let widget_on = self.widget_wanted.get();
        match Tray::spawn(widget_on, |action| {
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
    }

    pub fn show_main_sized(&self, size: LogicalSize) {
        if let Some(main) = self.main.get() {
            self.main_sized.set(true);
            main.window().set_size(size);
            if let Err(e) = main.show() {
                eprintln!("patch: can't show the window: {e}");
            }
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
                s.widget_pending.set(false);
                if s.widget_wanted.get() && s.widget.borrow().is_none() {
                    s.create_widget(&screen);
                }
            });
        } else if !want && have {
            self.destroy_widget();
        }
    }

    fn create_widget(&self, screen: &Screen) {
        let Some(app) = self.app() else { return };
        self.wayland.set(screen.wayland);
        let window = match OverlayWindow::new() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("patch: can't create the widget: {e}");
                return;
            }
        };
        app.attach_overlay(&window);
        // Alt+F4 on the widget turns it off for good, like the Settings toggle.
        let me = self.me.clone();
        window.window().on_close_requested(move || {
            if let Some(s) = me.upgrade() {
                s.on_tray(TrayAction::SetWidget(false));
            }
            CloseRequestResponse::HideWindow
        });
        let me = self.me.clone();
        let widget = Widget::new(window, move |x, y| {
            if let Some(s) = me.upgrade() {
                s.widget_moved(x, y);
            }
        });
        let place = widget.place(screen, self.state.borrow().widget);
        if let Err(e) = widget.show() {
            eprintln!("patch: can't show the widget: {e}");
            app.detach_overlay();
            return;
        }
        *self.widget.borrow_mut() = Some(widget);
        if let Some(place) = place {
            self.state.borrow_mut().widget = Some(place);
        }
        let me = self.me.clone();
        Timer::single_shot(Duration::from_millis(300), move || {
            if let Some(s) = me.upgrade() {
                if let Some(w) = s.widget.borrow().as_ref() {
                    w.settle(place);
                }
            }
        });
        if screen.wayland {
            self.wayland_widget_hint();
        }
    }

    fn destroy_widget(&self) {
        let Some(widget) = self.widget.borrow_mut().take() else { return };
        if let Some(place) = widget.place_now() {
            self.state.borrow_mut().widget = Some(place);
            self.save_state_soon();
        }
        widget.hide();
        if let Some(app) = self.app() {
            app.detach_overlay();
        }
    }

    fn widget_moved(&self, x: i32, y: i32) {
        let expanded = match self.widget.try_borrow() {
            Ok(w) => match w.as_ref() {
                Some(w) => w.expanded(),
                None => return,
            },
            Err(_) => return,
        };
        self.state.borrow_mut().widget = Some(patch_linux::state::WidgetPlace { x, y, expanded });
        self.save_state_soon();
    }

    fn wayland_widget_hint(&self) {
        if self.state.borrow().wayland_widget_hint {
            return;
        }
        let body = match self.opts.windowing {
            Windowing::Auto => {
                "Wayland doesn't let apps keep a window on top. From the next start Patch runs through \
                 XWayland while the widget is on, so it stays above other windows."
            }
            _ => {
                "Wayland doesn't let apps keep a window on top. Use your desktop's “Always on Top” \
                 window option for the widget, or start Patch with --x11."
            }
        };
        self.notify_user("Floating widget", body);
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
        if let Some(w) = self.widget.borrow().as_ref() {
            if let Some(place) = w.place_now() {
                self.state.borrow_mut().widget = Some(place);
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
        if let Some(widget) = self.widget.borrow_mut().take() {
            widget.hide();
        }
        if let Some(app) = self.app() {
            app.save_now();
            app.detach_overlay();
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
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_widget(on);
        }
        let me = self.me.clone();
        later(move || {
            if let Some(s) = me.upgrade() {
                s.sync_widget();
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
