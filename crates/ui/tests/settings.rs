//! The phone's opaque settings sheet must not paint or sample covered UI.

use patch_ui::{Actions, App, Editor, Host, MainWindow, Store, Theme};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Model};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

struct TestPlatform(Rc<MinimalSoftwareWindow>, Rc<Cell<Duration>>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.1.get()
    }
}

#[derive(Default)]
struct Phone(Cell<usize>);
impl Host for Phone {
    fn is_mobile(&self) -> bool {
        true
    }

    fn battery_unrestricted(&self) -> Option<bool> {
        self.0.set(self.0.get() + 1);
        Some(true)
    }
}

#[test]
fn settings_do_not_paint_covered_mobile_layers() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let now = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform(window.clone(), now.clone()))).unwrap();
    let dir = std::env::temp_dir().join(format!("patch-settings-{}", std::process::id()));
    let host = Rc::new(Phone::default());
    let app = App::new(dir.clone(), host.clone());
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    main.show().unwrap();
    window.set_size(PhysicalSize::new(412, 915));
    let store = main.global::<Store>();
    let actions = main.global::<Actions>();
    assert_eq!(store.get_themes().row_count(), 2);
    actions.invoke_set_look(1);
    assert!(main.global::<Theme>().get_light());
    actions.invoke_set_look(0);
    let mut pixels = vec![PremultipliedRgbaColor::default(); 412 * 915];
    let draw = |pixels: &mut [PremultipliedRgbaColor]| {
        let mut dirty = PhysicalSize::default();
        window.draw_if_needed(|r| dirty = r.render(pixels, 412).bounding_box_size());
        dirty
    };
    // Keep an editor draft underneath Settings, including its completed animation.
    main.global::<Actions>().invoke_new_task("".into(), -2);
    draw(&mut pixels);
    now.set(Duration::from_millis(300));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 0, "battery optimization is not checked with Settings closed");

    store.set_settings_open(true);
    draw(&mut pixels); // Start the enter animation before advancing the mock clock.
    now.set(Duration::from_millis(320));
    slint::platform::update_timers_and_animations();
    assert_ne!(draw(&mut pixels), PhysicalSize::default());
    assert_eq!(
        host.0.get(),
        1,
        "only the mobile settings panel checks battery optimization"
    );
    now.set(Duration::from_millis(580));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(draw(&mut pixels), PhysicalSize::default());

    window.dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(200.0, 500.0),
        delta_x: 0.0,
        delta_y: -120.0,
    });
    assert_ne!(
        draw(&mut pixels),
        PhysicalSize::default(),
        "Settings still scrolls"
    );
    assert_eq!(
        host.0.get(),
        1,
        "painting and scrolling do not check battery optimization"
    );

    now.set(Duration::from_millis(2580));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 2, "battery optimization refreshes once every two seconds");

    main.global::<Theme>()
        .set_backdrop_top(slint::Color::from_rgb_u8(255, 0, 255));
    store.set_title_main("Covered title".into());
    main.global::<Theme>()
        .set_dock(slint::Color::from_rgb_u8(255, 0, 255));
    main.global::<Editor>().set_title("Retained draft".into());
    assert_eq!(
        draw(&mut pixels),
        PhysicalSize::default(),
        "covered backdrop, calendar, dock and editor must not paint"
    );

    assert!(app.back());
    draw(&mut pixels); // Retain Settings while its exit animation runs.
    now.set(Duration::from_millis(2680));
    slint::platform::update_timers_and_animations();
    assert_ne!(
        draw(&mut pixels),
        PhysicalSize::default(),
        "closing Settings animates over the restored layers"
    );
    assert_eq!(
        store.get_title_main(),
        "Covered title",
        "hiding retains the underlying state"
    );
    assert!(
        store.get_editor_open(),
        "closing Settings retains the editor"
    );
    assert_eq!(main.global::<Editor>().get_title(), "Retained draft");
    now.set(Duration::from_millis(2780));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    now.set(Duration::from_millis(4780));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 2, "closing Settings stops its background timer");
    main.global::<Actions>().invoke_quick_add("Undo expiry".into());
    app.save_now();
    let id = patch_core::Doc::load_or_new(&dir.join("doc.json")).tasks().iter()
        .find(|t| t.title == "Undo expiry").unwrap().id.clone();
    main.global::<Actions>().invoke_delete_task(id.clone().into());
    assert_eq!(store.get_toast_action(), "Undo");
    now.set(Duration::from_millis(9781));
    slint::platform::update_timers_and_animations();
    assert_eq!(store.get_toast_action(), "");
    main.global::<Actions>().invoke_undo_delete();
    app.save_now();
    assert!(patch_core::Doc::load_or_new(&dir.join("doc.json")).task(&id).is_none());
    store.set_settings_open(true);
    store.set_environment_page(2);
    store.set_scan_active(true);
    assert!(app.back());
    assert_eq!(store.get_environment_page(), 1);
    assert!(store.get_settings_open());
    assert!(!store.get_scan_active());
    assert!(app.back());
    assert_eq!(store.get_environment_page(), 0);
    assert!(app.back());
    assert!(!store.get_settings_open());
    drop(main);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
