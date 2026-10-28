use chrono::Datelike;
use patch_ui::{Actions, App, Host, MainWindow, OverlayWindow, Store, Theme};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct TestPlatform(RefCell<Vec<Rc<MinimalSoftwareWindow>>>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(w.clone());
        Ok(w)
    }
}

#[derive(Default)]
struct Desktop(RefCell<Vec<Option<String>>>);

impl Host for Desktop {
    fn open_app(&self, id: Option<&str>) {
        self.0.borrow_mut().push(id.map(str::to_owned));
    }
}

fn render(w: &MinimalSoftwareWindow, size: u32) -> Vec<PremultipliedRgbaColor> {
    w.set_size(PhysicalSize::new(size, size));
    slint::platform::update_timers_and_animations();
    let mut pixels = vec![PremultipliedRgbaColor::default(); (size * size) as usize];
    w.request_redraw();
    w.draw_if_needed(|r| {
        r.render(&mut pixels, size as usize);
    });
    pixels
}

fn click(w: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
}

#[test]
fn independent_pet_and_desk_share_updates_and_open_tasks() {
    let dir = std::env::temp_dir().join(format!("patch-ui-desk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let platform = Rc::new(TestPlatform::default());
    struct Shared(Rc<TestPlatform>);
    impl Platform for Shared {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            self.0.create_window_adapter()
        }
    }
    slint::platform::set_platform(Box::new(Shared(platform.clone()))).unwrap();
    let host = Rc::new(Desktop::default());
    let app = App::new(dir.clone(), host.clone());
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    let actions = main.global::<Actions>();
    let pet = OverlayWindow::new().unwrap();
    app.attach_overlay(&pet);
    pet.show().unwrap();
    let pw = platform.0.borrow()[1].clone();

    assert_eq!(pet.get_mode(), 0);
    for mode in [0, 1] {
        pet.set_mode(mode);
        actions.invoke_overlay_expand(false);
        render(&pw, 60);
        click(&pw, 30.0, 30.0);
        assert!(pet.global::<Store>().get_overlay_expanded());
        assert!(host.0.borrow().is_empty(), "pet click expands without opening the app");
        render(&pw, 340);
        click(&pw, 272.0, 30.0);
        assert_eq!(host.0.borrow_mut().pop(), Some(None));
        click(&pw, 312.0, 30.0);
        assert!(!pet.global::<Store>().get_overlay_expanded());
    }

    let desk = OverlayWindow::new().unwrap();
    desk.set_mode(2);
    app.attach_desk(&desk);
    desk.show().unwrap();
    let dw = platform.0.borrow()[2].clone();
    let pixels = render(&dw, 352);
    for (x, y) in [(85, 85), (267, 85), (85, 267), (267, 267)] {
        assert!(pixels[y * 352 + x].alpha > 0, "each tile has a visible surface");
    }
    for (x, y) in [(176, 85), (85, 176), (176, 176), (0, 0)] {
        assert_eq!(pixels[y * 352 + x].alpha, 0, "gaps and rounded corners stay transparent");
    }
    click(&dw, 85.0, 85.0);
    assert_eq!(host.0.borrow_mut().pop(), Some(None));

    actions.invoke_quick_add_today("Call the plumber".into());
    actions.invoke_quick_add("Review tomorrow 10am".into());
    for (items, x) in [(desk.global::<Store>().get_today_items(), 20.0), (desk.global::<Store>().get_up_next(), 202.0)] {
        let id = items.row_data(0).expect("task model reaches desk").id;
        render(&dw, 352);
        click(&dw, x, 230.0);
        assert_eq!(host.0.borrow_mut().pop(), Some(Some(id.to_string())));
    }
    actions.invoke_set_look(1);
    assert_eq!(desk.global::<Store>().get_theme(), 1);
    assert_eq!(desk.global::<Theme>().get_accent(), pet.global::<Theme>().get_accent());
    assert_eq!(desk.global::<Store>().get_today_label(), main.global::<Store>().get_today_label());
    assert_eq!(desk.global::<Store>().get_today_day(), chrono::Local::now().day() as i32);
    app.detach_desk();
    actions.invoke_set_look(0);
    assert_eq!(desk.global::<Store>().get_theme(), 1);
    assert_eq!(pet.global::<Store>().get_theme(), 0);
    app.attach_desk(&desk);
    assert_eq!(desk.global::<Store>().get_theme(), 0);
    app.save_now();
    let _ = std::fs::remove_dir_all(dir);
}
