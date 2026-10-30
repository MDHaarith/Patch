use patch_ui::{Actions, MainWindow, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    time: Rc<Cell<Duration>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.time.get()
    }
}

#[test]
fn calendar_swipes_navigate_only_after_a_horizontal_threshold() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let time = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform { window: window.clone(), time: time.clone() })).unwrap();
    let main = MainWindow::new().unwrap();
    let store = main.global::<Store>();
    store.set_mobile(true);
    main.show().unwrap();
    window.set_size(PhysicalSize::new(412, 915));
    let navigation = Rc::new(RefCell::new(Vec::new()));
    main.global::<Actions>().on_nav({
        let navigation = navigation.clone();
        move |direction| navigation.borrow_mut().push(direction)
    });
    let render = || {
        time.set(time.get() + Duration::from_millis(250));
        slint::platform::update_timers_and_animations();
        let mut pixels = vec![PremultipliedRgbaColor::default(); 412 * 915];
        window.request_redraw();
        window.draw_if_needed(|r| { r.render(&mut pixels, 412); });
    };
    let drag = |dx: f32, dy: f32| {
        let start = LogicalPosition::new(206.0, 450.0);
        window.dispatch_event(WindowEvent::PointerPressed { position: start, button: PointerEventButton::Left });
        for step in 1..=10 {
            window.dispatch_event(WindowEvent::PointerMoved {
                position: LogicalPosition::new(start.x + dx * step as f32 / 10.0, start.y + dy * step as f32 / 10.0),
            });
            render();
        }
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(start.x + dx, start.y + dy), button: PointerEventButton::Left,
        });
        render();
    };
    for view in 0..=2 {
        store.set_view(view);
        render();
        render();
        navigation.borrow_mut().clear();
        drag(-130.0, 0.0);
        drag(130.0, 0.0);
        assert_eq!(*navigation.borrow(), vec![1, -1], "view {view}: both directions");
        drag(-40.0, 0.0);
        drag(0.0, -130.0);
        assert_eq!(*navigation.borrow(), vec![1, -1], "view {view}: short and vertical drags stay put");
    }
    store.set_mobile(false);
    drag(-130.0, 0.0);
    assert_eq!(*navigation.borrow(), vec![1, -1], "desktop leaves dragging to the calendar");
}
