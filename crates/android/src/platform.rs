//! The process-wide Slint platform. Every surface, the activity's and the
//! floating overlay's, gets a [`SurfaceWindow`]; the runtime picks the freshly
//! created adapter up right after instantiating a component.

use crate::runtime::{Bridge, Msg};
use crate::window::SurfaceWindow;
use slint::platform::{Clipboard, EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PlatformError};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

pub type Created = Rc<RefCell<Vec<Rc<SurfaceWindow>>>>;

pub struct PatchPlatform {
    created: Created,
    wake: Sender<Msg>,
    bridge: Rc<dyn Bridge>,
}

impl PatchPlatform {
    pub fn new(created: Created, wake: Sender<Msg>, bridge: Rc<dyn Bridge>) -> PatchPlatform {
        PatchPlatform {
            created,
            wake,
            bridge,
        }
    }
}

impl Platform for PatchPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = SurfaceWindow::new();
        self.created.borrow_mut().push(window.clone());
        Ok(window)
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy(self.wake.clone())))
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            self.bridge.set_clipboard(text);
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        match clipboard {
            Clipboard::DefaultClipboard => self.bridge.clipboard(),
            _ => None,
        }
    }
}

struct Proxy(Sender<Msg>);

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        // The UI thread lives as long as the process; Android decides when
        // that ends.
        Ok(())
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), EventLoopError> {
        self.0
            .send(Msg::Invoke(event))
            .map_err(|_| EventLoopError::EventLoopTerminated)
    }
}
