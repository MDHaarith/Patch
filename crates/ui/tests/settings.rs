//! The phone's opaque settings sheet must not paint or sample covered UI.

use patch_ui::{Actions, App, Editor, Host, MainWindow, Store, Theme};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
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
    let mut pixels = vec![PremultipliedRgbaColor::default(); 412 * 915];
    let draw = |pixels: &mut [PremultipliedRgbaColor]| {
        let mut dirty = PhysicalSize::default();
        window.draw_if_needed(|r| dirty = r.render(pixels, 412).bounding_box_size());
        dirty
    };
    // Keep an editor draft underneath Settings, including its completed animation.
    main.global::<Actions>().invoke_new_task("".into(), -2);
    draw(&mut pixels);
    now.set(Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 0, "usage is not sampled with Settings closed");

    store.set_settings_open(true);
    assert_ne!(draw(&mut pixels), PhysicalSize::default());
    assert_eq!(
        host.0.get(),
        1,
        "only the mobile settings panel samples usage"
    );
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
        "painting and scrolling do not sample usage"
    );

    now.set(Duration::from_millis(2250));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 2, "usage refreshes once every two seconds");

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
    assert_ne!(
        draw(&mut pixels),
        PhysicalSize::default(),
        "closing Settings must restore the layers"
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
    now.set(Duration::from_millis(4250));
    slint::platform::update_timers_and_animations();
    draw(&mut pixels);
    assert_eq!(host.0.get(), 2, "closing Settings stops its usage timer");
    drop(main);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
