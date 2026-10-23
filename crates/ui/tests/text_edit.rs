//! Shared fields keep wrapped text visible and use the platform clipboard.
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Clipboard, Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { Field } from "../ui/widgets.slint";
    export component TextEditingWindow inherits Window {
        in-out property <string> task-title <=> title.text;
        in-out property <string> notes <=> notes.text;
        out property <length> title-height: title.height;
        out property <length> notes-height: notes.height;
        out property <int> accepted;
        public function focus-title() { title.focus-input(); }
        public function focus-notes() { notes.focus-input(); }
        VerticalLayout {
            alignment: start;
            title := Field { accepted => { root.accepted += 1; } }
            notes := Field { multiline: true; min-height: 96px; }
        }
    }
}

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    clipboard: Rc<RefCell<String>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if clipboard == Clipboard::DefaultClipboard {
            *self.clipboard.borrow_mut() = text.into();
        }
    }
    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        (clipboard == Clipboard::DefaultClipboard).then(|| self.clipboard.borrow().clone())
    }
}

#[test]
fn wrapped_fields_and_clipboard() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let clipboard = Rc::new(RefCell::new(String::new()));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: window.clone(),
        clipboard: clipboard.clone(),
    }))
    .unwrap();
    let ui = TextEditingWindow::new().unwrap();
    ui.show().unwrap();
    window.set_size(PhysicalSize::new(240, 600));
    let render = || {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(&mut vec![PremultipliedRgbaColor::default(); 240 * 600], 240);
        });
    };
    render();
    let empty_height = ui.get_title_height();
    let long = "Plan the weekend with friends and family ".repeat(3);
    ui.set_task_title(long.clone().into());
    ui.set_notes(format!("{long}\n{long}").into());
    render();
    assert!(
        ui.get_title_height() > empty_height * 2.0,
        "title grows with soft wraps"
    );
    assert!(
        ui.get_notes_height() > ui.get_title_height(),
        "notes include every wrapped line and newline"
    );

    let key = |text: slint::SharedString| {
        window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.dispatch_event(WindowEvent::KeyReleased { text });
    };
    let shortcut = |text: &str| {
        window.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        key(text.into());
        window.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        render();
    };
    ui.invoke_focus_title();
    shortcut("a");
    shortcut("c");
    assert_eq!(*clipboard.borrow(), long);
    shortcut("x");
    assert_eq!(ui.get_task_title(), "");
    shortcut("v");
    assert_eq!(ui.get_task_title(), long);
    key(Key::Return.into());
    assert_eq!(ui.get_accepted(), 1, "Enter still submits a wrapping title");
    assert_eq!(ui.get_task_title(), long);
    ui.invoke_focus_notes();
    shortcut("a");
    key(Key::Return.into());
    assert_eq!(ui.get_notes(), "\n", "Enter inserts a newline in notes");

    ui.set_task_title(long.repeat(20).into());
    render();
    assert!(
        ui.get_title_height() <= 160.0,
        "large pastes scroll inside the quick-add field"
    );
    assert_eq!(
        ui.get_task_title(),
        long.repeat(20),
        "wrapping never truncates the text"
    );
}
