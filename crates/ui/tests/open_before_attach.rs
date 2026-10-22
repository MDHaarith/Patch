//! A task opened before the main window exists (tapped in the Android
//! overlay while the app was closed) must show once the window attaches.

use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::{Doc, Task};
use patch_ui::{App, Editor, Host, MainWindow, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::rc::Rc;

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

struct Phone;
impl Host for Phone {
    fn is_mobile(&self) -> bool {
        true
    }
}

#[test]
fn task_opened_before_the_main_window_is_shown_on_attach() {
    let dir = std::env::temp_dir().join(format!("patch-ui-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let id = {
        let mut d = Doc::with_node("seed");
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.title = "Call the plumber".into();
        let id = d.add_task(t);
        d.save(&dir.join("doc.json")).unwrap();
        id
    };
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();

    let app = App::new(dir, Rc::new(Phone));
    app.open_task(&id);
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);

    assert!(main.global::<Store>().get_editor_open());
    assert_eq!(main.global::<Editor>().get_title(), "Call the plumber");
    assert_eq!(main.global::<Editor>().get_id(), id.as_str());
}
