//! Drives the real UI with synthetic pointer and keyboard input on a
//! software-rendered window and checks the resulting document. Layout
//! coordinates follow the desktop shell at 1440×900.

use chrono::{Datelike, Duration};
use patch_core::doc::{COL_DOING, COL_DONE, COL_TODO, DEFAULT_LIST, LIST_WORK};
use patch_core::time::{hm, today};
use patch_core::{Doc, Task};
use patch_ui::{Actions, App, Host, MainWindow, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration as StdDuration;

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

struct Desktop;
impl Host for Desktop {}

struct Ui {
    w: Rc<MinimalSoftwareWindow>,
}

impl Ui {
    fn settle(&self) {
        for _ in 0..4 {
            slint::platform::update_timers_and_animations();
            std::thread::sleep(StdDuration::from_millis(25));
        }
        // Rendering runs layout and change callbacks like the real loop.
        let mut buf = vec![PremultipliedRgbaColor::default(); 1440 * 900];
        self.w.request_redraw();
        self.w.draw_if_needed(|r| {
            r.render(&mut buf, 1440);
        });
        slint::platform::update_timers_and_animations();
    }

    fn ev(&self, e: WindowEvent) {
        self.w.dispatch_event(e);
    }

    fn press(&self, x: f32, y: f32) {
        self.ev(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
        self.ev(WindowEvent::PointerPressed { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
    }

    fn release(&self, x: f32, y: f32) {
        self.ev(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
        self.settle();
    }

    fn click(&self, x: f32, y: f32) {
        self.press(x, y);
        self.release(x, y);
    }

    fn double_click(&self, x: f32, y: f32) {
        self.press(x, y);
        self.ev(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
        self.press(x, y);
        self.release(x, y);
    }

    fn drag(&self, from: (f32, f32), to: (f32, f32)) {
        self.press(from.0, from.1);
        for i in 1..=12 {
            let t = i as f32 / 12.0;
            let p = LogicalPosition::new(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            self.ev(WindowEvent::PointerMoved { position: p });
        }
        self.release(to.0, to.1);
    }

    fn type_text(&self, s: &str) {
        for ch in s.chars() {
            let text: slint::SharedString = ch.to_string().into();
            self.ev(WindowEvent::KeyPressed { text: text.clone() });
            self.ev(WindowEvent::KeyReleased { text });
        }
        self.settle();
    }

    fn key(&self, k: Key) {
        let text: slint::SharedString = k.into();
        self.ev(WindowEvent::KeyPressed { text: text.clone() });
        self.ev(WindowEvent::KeyReleased { text });
        self.settle();
    }
}

fn doc(app: &App, dir: &Path) -> Doc {
    app.save_now();
    Doc::load_or_new(&dir.join("doc.json"))
}

fn find(d: &Doc, title: &str) -> Task {
    d.tasks().into_iter().find(|t| t.title == title).unwrap_or_else(|| panic!("no task {title:?}"))
}

// Desktop shell geometry
const MAIN_X: f32 = 268.0;
const MAIN_W: f32 = 1162.0;
const CONTENT_Y: f32 = 119.0;
const CONTENT_H: f32 = 771.0;

#[test]
fn desktop_interactions() {
    let dir = std::env::temp_dir().join(format!("patch-ui-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    {
        let mut d = Doc::with_node("seed");
        for title in ["Alpha", "Bravo", "Charlie"] {
            let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
            t.title = title.into();
            d.add_task(t);
        }
        d.save(&dir.join("doc.json")).unwrap();
        std::fs::write(dir.join("prefs.json"), r#"{"view":3}"#).unwrap();
    }

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let ui = Ui { w: window.clone() };
    let app = App::new(dir.clone(), Rc::new(Desktop));
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    main.show().unwrap();
    window.set_size(PhysicalSize::new(1440, 900));
    ui.settle();
    assert_eq!(main.global::<Store>().get_view(), 3, "board view restored from prefs");

    // --- Board: drag the first card into "In Progress".
    let col_x = |c: f32| MAIN_X + 16.0 + c * 298.0;
    let card_y = |k: f32| CONTENT_Y + 16.0 + 44.0 + k * 86.0;
    ui.drag((col_x(0.0) + 80.0, card_y(0.0) + 30.0), (col_x(1.0) + 80.0, card_y(0.0) + 30.0));
    let d = doc(&app, &dir);
    assert_eq!(find(&d, "Alpha").column, COL_DOING, "card dropped into In Progress");

    // Reorder inside a column: drag Charlie (now 2nd) above Bravo.
    ui.drag((col_x(0.0) + 80.0, card_y(1.0) + 30.0), (col_x(0.0) + 80.0, card_y(0.0) + 10.0));
    let order: Vec<String> = doc(&app, &dir).tasks().into_iter().filter(|t| t.column == COL_TODO).map(|t| t.title).collect();
    assert_eq!(order, vec!["Charlie", "Bravo"]);

    // Tick the first card's checkbox: it completes and moves to Done.
    ui.click(col_x(0.0) + 10.0 + 266.0 - 12.0 - 9.0, card_y(0.0) + 19.0);
    let charlie = find(&doc(&app, &dir), "Charlie");
    assert!(charlie.done);
    assert_eq!(charlie.column, COL_DONE);

    // --- Quick add from the toolbar with natural language.
    let day = today();
    ui.click(MAIN_X + 120.0, 35.0);
    ui.type_text(&format!("Dentist {} 3pm #work !!!", day.format("%Y-%m-%d")));
    ui.key(Key::Return);
    let dentist = find(&doc(&app, &dir), "Dentist");
    assert_eq!(dentist.start, Some(day.and_time(hm(15, 0))));
    assert_eq!(dentist.list, LIST_WORK);
    assert_eq!(dentist.priority, 3);

    // --- Month: drag Dentist two days later (same week row if possible).
    main.global::<Actions>().invoke_set_view(2);
    ui.settle();
    let grid0 = patch_core::time::month_grid(day, true)[0];
    let idx = (day - grid0).num_days() as f32;
    let target_offset: i64 = if day.weekday().num_days_from_monday() <= 4 { 2 } else { -2 };
    let cell_w = MAIN_W / 7.0;
    let cell_h = (CONTENT_H - 30.0) / 6.0;
    let cell_xy = |i: f32| (MAIN_X + (i % 7.0) * cell_w, CONTENT_Y + 30.0 + (i / 7.0).floor() * cell_h);
    let (cx, cy) = cell_xy(idx);
    let (tx, ty) = cell_xy(idx + target_offset as f32);
    ui.drag((cx + 30.0, cy + 31.0 + 9.0), (tx + cell_w / 2.0, ty + cell_h / 2.0));
    let moved = find(&doc(&app, &dir), "Dentist");
    let new_day = day + Duration::days(target_offset);
    assert_eq!(moved.start, Some(new_day.and_time(hm(15, 0))), "month drag keeps the time");

    // --- Week: drag the block one hour later.
    main.global::<Actions>().invoke_select_date(new_day.to_string().into());
    main.global::<Actions>().invoke_set_view(1);
    ui.settle();
    let di = new_day.weekday().num_days_from_monday() as f32;
    let col_w = (MAIN_W - 56.0) / 7.0;
    let body_y = CONTENT_Y + 44.0 + 30.0 + 1.0;
    let hour_h = 46.0;
    let block_y = body_y + 10.0 + 15.0 * hour_h - 7.5 * hour_h + 12.0;
    let block_x = MAIN_X + 56.0 + di * col_w + 30.0;
    ui.drag((block_x, block_y), (block_x, block_y + hour_h));
    let later = find(&doc(&app, &dir), "Dentist");
    assert_eq!(later.start, Some(new_day.and_time(hm(16, 0))), "week drag snaps to the hour");
    assert_eq!(later.span_end(), Some(new_day.and_time(hm(17, 0))), "duration preserved");

    // Resize from the bottom edge by 30 minutes.
    let bottom = body_y + 10.0 + 17.0 * hour_h - 7.5 * hour_h - 3.0;
    ui.drag((block_x, bottom), (block_x, bottom + hour_h / 2.0));
    let resized = find(&doc(&app, &dir), "Dentist");
    assert_eq!(resized.end, Some(new_day.and_time(hm(17, 30))), "resize snaps to 15 minutes");

    // --- Editor: new task by double-clicking an empty month day, then use
    // the range picker to make it span three days.
    main.global::<Actions>().invoke_set_view(2);
    ui.settle();
    let empty = (0..42)
        .map(|i| grid0 + Duration::days(i))
        .find(|d| d.month() == day.month() && (*d - day).num_days().abs() > 3 && d.day() <= 20)
        .unwrap();
    let (ex, ey) = cell_xy((empty - grid0).num_days() as f32);
    ui.double_click(ex + cell_w / 2.0, ey + cell_h - 12.0);
    assert!(main.global::<Store>().get_editor_open(), "double-click opens the editor");
    // Let the inspector finish sliding in before clicking inside it.
    std::thread::sleep(StdDuration::from_millis(250));
    ui.settle();
    ui.type_text("Conference");
    // Picker cells: find them by key through the model and click via the
    // picker geometry inside the 360px inspector.
    let ed = main.global::<patch_ui::Editor>();
    let cells = ed.get_picker_cells();
    use slint::Model;
    let pos_of = |key: &str| (0..cells.row_count()).find(|i| cells.row_data(*i).unwrap().key == key).unwrap() as f32;
    let start = empty;
    let end = empty + Duration::days(2);
    assert!(main.global::<patch_ui::Editor>().get_all_day());
    let inspector_x = 1440.0 - 10.0 - 360.0;
    let picker_x = inspector_x + 16.0 + 16.0;
    let picker_w = 360.0 - 32.0 - 32.0;
    // Inspector header 52 + divider 1 + scroll padding 16; blocks above the
    // picker: title(30)+18, list group(13+8+28)+18, status(13+8+28)+18,
    // priority(13+8+28)+18, schedule label 13+8, two toggle rows (22+8)*2.
    let picker_top = 10.0 + 53.0 + 16.0 + 48.0 + 67.0 + 67.0 + 67.0 + 21.0 + 60.0;
    let days_top = picker_top + 14.0 + 28.0 + 6.0 + 16.0 + 6.0;
    let cell_xy_p = |i: f32| (picker_x + (i % 7.0 + 0.5) * picker_w / 7.0, days_top + ((i / 7.0).floor() + 0.5) * 30.0);
    let (sx, sy) = cell_xy_p(pos_of(&start.to_string()));
    let (qx, qy) = cell_xy_p(pos_of(&end.to_string()));
    ui.click(sx, sy);
    ui.click(qx, qy);
    assert_eq!(ed.get_start_key(), start.to_string());
    assert_eq!(ed.get_end_key(), end.to_string(), "second tap extends the range");
    // Save with the Add button.
    ui.click(1440.0 - 10.0 - 12.0 - 22.0, 10.0 + 26.0);
    assert!(!main.global::<Store>().get_editor_open());
    let conf = find(&doc(&app, &dir), "Conference");
    assert!(conf.all_day);
    assert_eq!(conf.start.map(|s| s.date()), Some(start));
    assert_eq!(conf.end.map(|s| s.date()), Some(end));

    // Both deletion routes offer the same undo without losing task fields.
    main.global::<Actions>().invoke_delete_task(conf.id.clone().into());
    assert!(doc(&app, &dir).task(&conf.id).is_none());
    assert_eq!(main.global::<Store>().get_toast(), "Task deleted");
    assert_eq!(main.global::<Store>().get_toast_action(), "Undo");
    main.global::<Actions>().invoke_undo_delete();
    assert_eq!(doc(&app, &dir).task(&conf.id), Some(conf.clone()));
    app.open_task(&conf.id);
    main.global::<Actions>().invoke_editor_delete();
    assert_eq!(main.global::<Store>().get_toast_action(), "Undo");
    main.global::<Actions>().invoke_undo_delete();
    assert_eq!(doc(&app, &dir).task(&conf.id), Some(conf.clone()));
    App::complete_current_task(&conf.id);
    assert!(doc(&app, &dir).task(&conf.id).unwrap().done);
}
