use chrono::Duration;
use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::model::Repeat;
use patch_core::time::now_local;
use patch_core::{Doc, Task};
use patch_ui::{Actions, App, Editor, Host, MainWindow, Reminder, Store, Theme};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::rc::Rc;

struct TestPlatform;
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

#[derive(Default)]
struct TestHost {
    reminders: RefCell<Vec<Reminder>>,
    opened: RefCell<Vec<String>>,
}
impl Host for TestHost {
    fn schedule_reminders(&self, reminders: &[Reminder]) -> bool {
        *self.reminders.borrow_mut() = reminders.to_vec();
        true
    }
    fn open_url(&self, url: &str) {
        self.opened.borrow_mut().push(url.into());
    }
}

fn rgb(r: u8, g: u8, b: u8) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}

fn task(doc: &mut Doc, title: &str, start: chrono::NaiveDateTime, repeat: Repeat) -> String {
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.title = title.into();
    t.start = Some(start);
    t.repeat = repeat;
    t.notes = "Docs at https://example.com/a.".into();
    doc.add_task(t)
}

#[test]
fn focus_plan_skip_catch_up_links_and_appearance() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let dir = std::env::temp_dir().join(format!("patch-daily-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let now = now_local();
    let mut doc = Doc::with_node("daily");
    let overdue = task(&mut doc, "Overdue", now - Duration::days(3), Repeat::None);
    let daily = task(&mut doc, "Daily", now + Duration::hours(1), Repeat::Daily);
    let daily_start = doc.task(&daily).unwrap().start.unwrap();
    doc.save(&dir.join("doc.json")).unwrap();

    let host = Rc::new(TestHost::default());
    let app = App::new(dir.clone(), host.clone());
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    let actions = main.global::<Actions>();
    let store = main.global::<Store>();
    let editor = main.global::<Editor>();
    let read = || {
        app.save_now();
        Doc::load_or_new(&dir.join("doc.json"))
    };

    // The very first launch doesn't interrupt; asking for the plan shows what's on.
    assert!(!store.get_plan_open(), "no prompt on first launch");
    app.show_plan();
    assert!(store.get_plan_open());
    assert_eq!(store.get_plan_overdue(), 1);

    // Catching up moves the task, closes the sheet, and undoes as one step.
    actions.invoke_catch_up();
    assert!(!store.get_plan_open());
    assert!(!read().task(&overdue).unwrap().is_overdue(now));
    assert_eq!(store.get_toast_action().as_str(), "Undo");
    actions.invoke_undo();
    assert!(read().task(&overdue).unwrap().is_overdue(now));

    // Notes offer their links; only those the parser allows reach the host.
    actions.invoke_open_task(daily.clone().into());
    assert_eq!(editor.get_links().row_data(0).unwrap().as_str(), "https://example.com/a");
    actions.invoke_notes_edited("nothing here".into());
    assert_eq!(editor.get_links().row_count(), 0);
    actions.invoke_open_link("https://example.com/a".into());
    assert_eq!(host.opened.borrow().len(), 1);

    // Focus starts from the editor, schedules its alert and counts minutes when stopped.
    actions.invoke_focus_start(25);
    assert!(store.get_focus_active());
    assert_eq!(store.get_focus_title().as_str(), "Daily");
    let left = store.get_focus_left();
    assert!(left.as_str() == "25:00" || left.starts_with("24:"), "{left}");
    assert!(host.reminders.borrow().iter().any(|r| r.key.contains("@focus@") && r.title == "Focus finished"));
    assert!(App::current_task_open(&host.reminders.borrow().iter().find(|r| r.key.contains("@focus@")).unwrap().key) == Some(true));
    actions.invoke_focus_stop();
    assert!(!store.get_focus_active());
    assert!(!host.reminders.borrow().iter().any(|r| r.key.contains("@focus@")), "stopping cancels the alert");

    // Skipping rolls forward and isn't a completion.
    actions.invoke_open_task(daily.clone().into());
    actions.invoke_skip_occurrence();
    let skipped = read();
    assert_eq!(skipped.task(&daily).unwrap().start, Some(daily_start + Duration::days(1)));
    assert_eq!(skipped.stats(now.date()).today, 0);

    // Completing feeds the progress card.
    actions.invoke_toggle_done(daily.clone().into());
    assert_eq!(store.get_stat_today(), 1);
    assert_eq!(store.get_stat_streak(), 1);
    assert_eq!(store.get_stat_week().row_data(6), Some(1));

    actions.invoke_set_daily_plan(false);
    assert!(!store.get_daily_plan());

    // Accent and text size apply immediately and persist; pale accents get dark ink.
    actions.invoke_set_accent(3);
    actions.invoke_set_text_size(2);
    let theme = main.global::<Theme>();
    assert_eq!(theme.get_accent(), rgb(0xFF, 0xD6, 0x0A));
    assert_eq!(theme.get_on_accent(), rgb(0, 0, 0));
    assert!((theme.get_text_scale() - 1.3).abs() < 0.001);
    assert!((theme.get_fs_m() - 13.0 * 1.3).abs() < 0.01);
    actions.invoke_set_look(1);
    assert_eq!(theme.get_accent(), rgb(0xFF, 0xD6, 0x0A), "changing look keeps the accent");
    actions.invoke_set_accent(0);
    assert_eq!(theme.get_accent(), rgb(0x00, 0x7A, 0xFF));
    assert_eq!(theme.get_on_accent(), rgb(0xFF, 0xFF, 0xFF));
    let prefs = std::fs::read_to_string(dir.join("prefs.json")).unwrap();
    assert!(prefs.contains("\"text_size\": 2") || prefs.contains("\"text_size\":2"), "{prefs}");
    std::fs::remove_dir_all(&dir).ok();
}
