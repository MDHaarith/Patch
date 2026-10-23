//! Window layers on X11 (and XWayland): "above everything", "on the desktop
//! below everything", on all workspaces, out of the taskbar.
//!
//! winit only offers `set_window_level`, which sends the EWMH request once.
//! Window managers ignore such requests for a window that isn't mapped yet
//! (winit makes them at creation, before Slint shows the window), and winit
//! has nothing for "sticky" or "skip taskbar". So the shell talks EWMH itself
//! over a second X connection: while a window is withdrawn it writes
//! `_NET_WM_STATE` directly (the window manager reads it when mapping); once
//! it is mapped it sends the `_NET_WM_STATE` client messages, and re-checks
//! later in case the window manager dropped a state.
//!
//! KWin, Mutter, Xfwm, Openbox and the other EWMH window managers keep an
//! `_ABOVE` window above normal ones even while another window is active,
//! and a `_BELOW` window above the desktop but under normal windows. (The
//! `_DESKTOP` window type would survive Show Desktop, but on KWin the
//! board's transparent corners then showed as a solid box.)

use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, MapState, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Normal,
    /// Above other apps, even while one of them is active.
    Above,
    /// On the desktop: below every normal window.
    Desktop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hints {
    pub layer: Layer,
    /// On every workspace.
    pub sticky: bool,
    /// Out of the taskbar, pager and task switcher.
    pub skip_taskbar: bool,
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_BELOW,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _KDE_NET_WM_STATE_SKIP_SWITCHER,
        _NET_WM_DESKTOP,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_SHOWING_DESKTOP,
        _NET_CURRENT_DESKTOP,
        _NET_CLIENT_LIST_STACKING,
        _NET_CLIENT_LIST,
        _NET_SUPPORTED,
    }
}

const REMOVE: u32 = 0;
const ADD: u32 = 1;
/// "Normal application" as the source of a `_NET_WM_STATE` request.
const SOURCE_APP: u32 = 1;
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

fn pet_visible(showing_desktop: bool, visible_app: Option<bool>) -> bool {
    !showing_desktop && visible_app.unwrap_or(true)
}

/// The X11 id of a Slint window, `None` on Wayland or before it exists.
pub fn x11_id(window: &slint::Window) -> Option<Window> {
    window
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
            RawWindowHandle::Xcb(h) => Some(h.window.get()),
            _ => None,
        })
        .flatten()
}

pub struct Wm {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    /// `_NET_SUPPORTED`: states this window manager knows. Others are never
    /// requested (Openbox, say, keeps "sticky" only as `_NET_WM_DESKTOP`).
    supported: Vec<u32>,
}

impl Wm {
    pub fn connect() -> Result<Wm, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots.get(screen).ok_or("no X screen")?.root;
        let atoms = Atoms::new(&conn).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        let supported = conn
            .get_property(false, root, atoms._NET_SUPPORTED, AtomEnum::ATOM, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default();
        Ok(Wm { conn, root, atoms, supported })
    }

    fn supports(&self, atom: u32) -> bool {
        // No list at all: a window manager that doesn't say; try everything.
        self.supported.is_empty() || self.supported.contains(&atom)
    }

    fn wanted(&self, h: Hints) -> (Vec<u32>, Vec<u32>) {
        let a = &self.atoms;
        let mut add = Vec::new();
        let mut remove = Vec::new();
        match h.layer {
            Layer::Above => {
                add.push(a._NET_WM_STATE_ABOVE);
                remove.push(a._NET_WM_STATE_BELOW);
            }
            Layer::Desktop => {
                add.push(a._NET_WM_STATE_BELOW);
                remove.push(a._NET_WM_STATE_ABOVE);
            }
            Layer::Normal => remove.extend([a._NET_WM_STATE_ABOVE, a._NET_WM_STATE_BELOW]),
        }
        if h.sticky {
            add.push(a._NET_WM_STATE_STICKY);
        }
        if h.skip_taskbar {
            add.extend([a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_SKIP_PAGER, a._KDE_NET_WM_STATE_SKIP_SWITCHER]);
        }
        add.retain(|a| self.supports(*a));
        (add, remove)
    }

    fn property(&self, win: Window, name: u32, type_: AtomEnum) -> Result<Vec<u32>, String> {
        let reply = self
            .conn
            .get_property(false, win, name, type_, 0, 64)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
    }

    fn map_state(&self, win: Window) -> Result<MapState, String> {
        let attrs =
            self.conn.get_window_attributes(win).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        Ok(attrs.map_state)
    }

    /// Missing EWMH workspace/client data leaves the pet visible.
    pub fn pet_visible(&self, own: &[Window]) -> Result<bool, String> {
        let a = &self.atoms;
        let showing = self.property(self.root, a._NET_SHOWING_DESKTOP, AtomEnum::CARDINAL)?.first() == Some(&1);
        if showing {
            return Ok(pet_visible(true, None));
        }
        let desktop = self.property(self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL)?;
        let Some(desktop) = desktop.first() else { return Ok(pet_visible(false, None)) };
        let clients = self
            .conn
            .get_property(false, self.root, a._NET_CLIENT_LIST_STACKING, AtomEnum::WINDOW, 0, u32::MAX)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let clients = if clients.type_ == 0 {
            self.conn
                .get_property(false, self.root, a._NET_CLIENT_LIST, AtomEnum::WINDOW, 0, u32::MAX)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
        } else {
            clients
        };
        let Some(clients) = clients.value32() else { return Ok(pet_visible(false, None)) };
        for win in clients.filter(|w| !own.contains(w)) {
            // Clients can disappear between reading the list and their properties.
            let visible = (|| -> Result<bool, String> {
                let workspace = self.property(win, a._NET_WM_DESKTOP, AtomEnum::CARDINAL)?;
                if !workspace.first().is_some_and(|d| d == desktop || *d == ALL_DESKTOPS) {
                    return Ok(false);
                }
                let types = self.property(win, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM)?;
                let states = self.property(win, a._NET_WM_STATE, AtomEnum::ATOM)?;
                Ok(!types.contains(&a._NET_WM_WINDOW_TYPE_DESKTOP)
                    && !types.contains(&a._NET_WM_WINDOW_TYPE_DOCK)
                    && !states.contains(&a._NET_WM_STATE_HIDDEN)
                    && self.map_state(win)? == MapState::VIEWABLE)
            })();
            if visible.unwrap_or(false) {
                return Ok(pet_visible(false, Some(true)));
            }
        }
        Ok(pet_visible(false, Some(false)))
    }

    fn request(&self, win: Window, type_: u32, data: [u32; 5]) -> Result<(), String> {
        let event = ClientMessageEvent::new(32, win, type_, data);
        self.conn
            .send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Brings `win` to `hints`. Missing states are always added; with
    /// `strict`, states of the other layer are removed too (only on an
    /// explicit change, so a periodic check never undoes the user's own
    /// "keep above" from the window menu). Returns whether anything was sent.
    pub fn apply(&self, win: Window, hints: Hints, strict: bool) -> Result<bool, String> {
        let (add, remove) = self.wanted(hints);
        let current = self.property(win, self.atoms._NET_WM_STATE, AtomEnum::ATOM)?;
        let missing: Vec<u32> = add.iter().copied().filter(|a| !current.contains(a)).collect();
        let extra: Vec<u32> =
            if strict { remove.iter().copied().filter(|a| current.contains(a)).collect() } else { Vec::new() };
        let all_desktops = !hints.sticky
            || !self.supports(self.atoms._NET_WM_DESKTOP)
            || self.property(win, self.atoms._NET_WM_DESKTOP, AtomEnum::CARDINAL)?.first() == Some(&ALL_DESKTOPS);
        if missing.is_empty() && extra.is_empty() && all_desktops {
            return Ok(false);
        }
        if self.map_state(win)? == MapState::UNMAPPED {
            // Withdrawn: the window manager reads the property when it maps it.
            let mut state: Vec<u32> = current.into_iter().filter(|a| !extra.contains(a)).collect();
            state.extend(&missing);
            self.conn
                .change_property32(PropMode::REPLACE, win, self.atoms._NET_WM_STATE, AtomEnum::ATOM, &state)
                .map_err(|e| e.to_string())?;
            if !all_desktops {
                self.conn
                    .change_property32(
                        PropMode::REPLACE,
                        win,
                        self.atoms._NET_WM_DESKTOP,
                        AtomEnum::CARDINAL,
                        &[ALL_DESKTOPS],
                    )
                    .map_err(|e| e.to_string())?;
            }
        } else {
            for pair in extra.chunks(2) {
                let second = pair.get(1).copied().unwrap_or(0);
                self.request(win, self.atoms._NET_WM_STATE, [REMOVE, pair[0], second, SOURCE_APP, 0])?;
            }
            for pair in missing.chunks(2) {
                let second = pair.get(1).copied().unwrap_or(0);
                self.request(win, self.atoms._NET_WM_STATE, [ADD, pair[0], second, SOURCE_APP, 0])?;
            }
            if !all_desktops {
                self.request(win, self.atoms._NET_WM_DESKTOP, [ALL_DESKTOPS, SOURCE_APP, 0, 0, 0])?;
            }
        }
        self.conn.flush().map_err(|e| e.to_string())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::pet_visible;

    #[test]
    fn pet_follows_apps_and_falls_back_without_ewmh() {
        for apps in [None, Some(false), Some(true)] {
            assert!(!pet_visible(true, apps));
        }
        assert!(!pet_visible(false, Some(false)));
        assert!(pet_visible(false, Some(true)));
        assert!(pet_visible(false, None));
    }
}
