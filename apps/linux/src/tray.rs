//! Tray icon through StatusNotifierItem (KDE, XFCE, Cinnamon, MATE, LXQt,
//! Budgie, and GNOME with the AppIndicator extension).
//!
//! Slint 1.18 has a `SystemTrayIcon` element, but it hard-codes the item id
//! and swallows "no tray host" errors. Talking to ksni directly lets the shell
//! know whether a tray is actually visible, which decides what closing the
//! main window does.

use crate::icon;
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{MenuItem, ToolTip};
use patch_linux::paths::APP_ID;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    SetWidget(bool),
    Quit,
}

type Dispatch = Arc<dyn Fn(TrayAction) + Send + Sync>;

struct Item {
    widget_on: bool,
    hub: Option<u16>,
    icon_name: String,
    pixmaps: Vec<ksni::Icon>,
    live: Arc<AtomicBool>,
    dispatch: Dispatch,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "Patch".into()
    }

    fn icon_name(&self) -> String {
        self.icon_name.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmaps.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let description = match self.hub {
            Some(port) => format!("Hosting the sync hub on port {port}"),
            None => String::new(),
        };
        ToolTip { title: "Patch".into(), description, ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.dispatch)(TrayAction::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Patch".into(),
                activate: Box::new(|t: &mut Item| (t.dispatch)(TrayAction::Open)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Floating widget".into(),
                checked: self.widget_on,
                activate: Box::new(|t: &mut Item| (t.dispatch)(TrayAction::SetWidget(!t.widget_on))),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit Patch".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Item| (t.dispatch)(TrayAction::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        self.live.store(true, Ordering::SeqCst);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.live.store(false, Ordering::SeqCst);
        // Keep the service: the tray host may come back (shell restart,
        // extension enabled later, login before the panel is up).
        true
    }
}

pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
    live: Arc<AtomicBool>,
}

impl Tray {
    /// Registers the tray icon. `dispatch` runs on the tray's D-Bus thread.
    pub fn spawn(widget_on: bool, dispatch: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Tray, String> {
        let live = Arc::new(AtomicBool::new(true));
        let item = Item {
            widget_on,
            hub: None,
            // Prefer the themed icon (sharp at any panel size) once installed.
            icon_name: if icon::themed_icon_installed() { APP_ID.into() } else { String::new() },
            pixmaps: icon::tray_pixmaps(),
            live: live.clone(),
            dispatch: Arc::new(dispatch),
        };
        // A missing StatusNotifierWatcher is reported through
        // `watcher_offline` instead of failing, so a tray host that starts
        // after us (autostart at login) still gets the icon.
        let handle = item.assume_sni_available(true).spawn().map_err(|e| e.to_string())?;
        Ok(Tray { handle, live })
    }

    /// Whether a tray host currently shows the icon.
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst) && !self.handle.is_closed()
    }

    pub fn set_widget(&self, on: bool) {
        self.handle.update(|t| t.widget_on = on);
    }

    pub fn set_hub(&self, port: Option<u16>) {
        self.handle.update(|t| t.hub = port);
    }

    pub fn shutdown(self) {
        self.handle.shutdown().wait();
    }
}
