use patch_ui::{Actions, BoardCol, MainWindow, Section, Store, TaskItem};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, VecModel};
use std::cell::{Cell, RefCell};
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

#[test]
fn phone_hold_release_selects_and_hold_move_still_drags() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let time = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform(window.clone(), time.clone()))).unwrap();
    let main = MainWindow::new().unwrap();
    let store = main.global::<Store>();
    store.set_mobile(true);
    store.set_view(3);
    let items = Rc::new(VecModel::from(vec![
        TaskItem {
            id: "one".into(),
            title: "First".into(),
            ..Default::default()
        },
        TaskItem {
            id: "two".into(),
            title: "Second".into(),
            ..Default::default()
        },
    ]));
    store.set_board(
        Rc::new(VecModel::from(vec![
            BoardCol {
                id: "todo".into(),
                name: "To Do".into(),
                count: 2,
                cards: items.clone().into(),
                ..Default::default()
            },
            BoardCol {
                id: "doing".into(),
                name: "Doing".into(),
                ..Default::default()
            },
        ]))
        .into(),
    );
    store.set_board_tallest(2);
    store.set_sections(
        Rc::new(VecModel::from(vec![Section {
            title: "Someday".into(),
            items: items.into(),
            ..Default::default()
        }]))
        .into(),
    );
    let selected = Rc::new(RefCell::new(Vec::new()));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let opened = Rc::new(RefCell::new(Vec::new()));
    let actions = main.global::<Actions>();
    actions.on_select_task({
        let selected = selected.clone();
        move |id, range| selected.borrow_mut().push((id.to_string(), range))
    });
    actions.on_drop_card({
        let dropped = dropped.clone();
        move |id, col, index| dropped.borrow_mut().push((id.to_string(), col, index))
    });
    actions.on_open_task({
        let opened = opened.clone();
        move |id| opened.borrow_mut().push(id.to_string())
    });
    main.show().unwrap();
    window.set_size(PhysicalSize::new(412, 915));
    let render = |ms: u64| {
        // Flickable forwards presses after a delay; both it and the hold timer need frames.
        let mut remaining = ms;
        loop {
            let step = remaining.min(50);
            time.set(time.get() + Duration::from_millis(step));
            slint::platform::update_timers_and_animations();
            let mut pixels = vec![PremultipliedRgbaColor::default(); 412 * 915];
            window.request_redraw();
            window.draw_if_needed(|r| {
                r.render(&mut pixels, 412);
            });
            remaining -= step;
            if remaining == 0 {
                break;
            }
        }
    };
    let press = |x, y| {
        let position = LogicalPosition::new(x, y);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    };
    let release = |x, y| {
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        })
    };
    render(0);
    render(600);
    press(100.0, 180.0);
    render(0);
    render(600);
    release(100.0, 180.0);
    render(0);
    assert_eq!(
        *selected.borrow(),
        [("one".into(), false)],
        "opened={:?}, dropped={:?}",
        opened.borrow(),
        dropped.borrow()
    );
    assert!(dropped.borrow().is_empty() && opened.borrow().is_empty());
    store.set_selection_mode(true);
    press(100.0, 275.0);
    release(100.0, 275.0);
    render(0);
    assert_eq!(selected.borrow().last(), Some(&("two".into(), false)));
    store.set_selection_mode(false);
    press(100.0, 180.0);
    render(0);
    render(600);
    window.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(390.0, 180.0),
    });
    render(1);
    release(390.0, 180.0);
    render(0);
    assert_eq!(*dropped.borrow(), [("one".into(), 1, 0)]);
    assert!(opened.borrow().is_empty());
    store.set_view(4);
    render(0);
    render(600);
    selected.borrow_mut().clear();
    press(100.0, 180.0);
    render(0);
    render(600);
    release(100.0, 180.0);
    render(0);
    assert_eq!(*selected.borrow(), [("one".into(), false)]);
    assert!(opened.borrow().is_empty());

    let history = Rc::new(RefCell::new(Vec::new()));
    actions.on_undo({
        let history = history.clone();
        move || history.borrow_mut().push("undo")
    });
    actions.on_redo({
        let history = history.clone();
        move || history.borrow_mut().push("redo")
    });
    store.set_mobile(false);
    main.invoke_refocus();
    let key = |text: slint::SharedString, pressed| {
        window.dispatch_event(if pressed {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        })
    };
    key(Key::Control.into(), true);
    key("z".into(), true);
    key("z".into(), false);
    key(Key::Shift.into(), true);
    key("Z".into(), true);
    key("Z".into(), false);
    key(Key::Shift.into(), false);
    key("y".into(), true);
    key("y".into(), false);
    key(Key::Control.into(), false);
    assert_eq!(*history.borrow(), ["undo", "redo", "redo"]);
}
