use chrono::Duration;
use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::{Doc, Task};
use patch_ui::{Actions, App, Editor, Host, MainWindow, OverlayWindow, Reminder, Store};
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
struct TestHost(RefCell<Vec<Reminder>>);
impl Host for TestHost {
    fn schedule_reminders(&self, reminders: &[Reminder]) -> bool {
        *self.0.borrow_mut() = reminders.to_vec();
        true
    }
}

#[test]
fn shared_actions_filter_bulk_undo_templates_archive_and_reminders() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let dir = std::env::temp_dir().join(format!("patch-features-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = Doc::with_node("features");
    let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
    task.title = "First".into();
    task.tags = vec!["rust".into()];
    task.start = Some(patch_core::time::now_local() + Duration::minutes(5));
    task.reminder = Some(0);
    task.reminders = vec![5];
    task.parent = Some("missing-parent".into());
    let first = doc.add_task(task.clone());
    task.parent = None;
    task.title = "Second".into();
    task.start = None;
    task.reminder = None;
    task.reminders.clear();
    let second = doc.add_task(task.clone());
    task.title = "Template".into();
    task.template = true;
    let template = doc.add_task(task.clone());
    task.template = false;
    task.title = "Archived".into();
    task.archived = true;
    let archived = doc.add_task(task);
    doc.save(&dir.join("doc.json")).unwrap();
    let host = Rc::new(TestHost::default());
    let app = App::new(dir.clone(), host.clone());
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    let overlay = OverlayWindow::new().unwrap();
    app.attach_overlay(&overlay);
    let actions = main.global::<Actions>();
    let store = main.global::<Store>();
    actions.invoke_set_view(4);
    let read = || {
        app.save_now();
        Doc::load_or_new(&dir.join("doc.json"))
    };
    let visible = || {
        store
            .get_sections()
            .iter()
            .flat_map(|s| s.items.iter().map(|t| t.id.to_string()).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    assert_eq!(visible().len(), 2);
    assert_eq!(host.0.borrow().len(), 2);
    assert_ne!(host.0.borrow()[0].key, host.0.borrow()[1].key);
    actions.invoke_open_task(first.clone().into());
    assert!(main
        .global::<Editor>()
        .get_parent_options()
        .iter()
        .any(|name| name == "Unavailable parent"));
    actions.invoke_editor_save();
    assert_eq!(
        read().task(&first).unwrap().parent.as_deref(),
        Some("missing-parent")
    );

    actions.invoke_select_task(first.clone().into(), false);
    actions.invoke_select_task(second.clone().into(), true);
    assert_eq!(store.get_selection_count(), 2);
    actions.invoke_bulk(4, "3".into());
    assert!(read()
        .tasks()
        .iter()
        .filter(|t| t.id == first || t.id == second)
        .all(|t| t.priority == 3));
    assert!(store.get_can_undo());
    actions.invoke_undo();
    assert!(store.get_can_redo());
    assert!(read()
        .tasks()
        .iter()
        .filter(|t| t.id == first || t.id == second)
        .all(|t| t.priority == 0));
    actions.invoke_redo();
    assert_eq!(read().task(&first).unwrap().priority, 3);

    store.set_filter_tags("missing".into());
    actions.invoke_filter_changed();
    assert!(visible().is_empty());
    store.set_view_name("Empty filter".into());
    actions.invoke_save_smart();
    assert_eq!(read().smart_views().len(), 1);
    let view = read().smart_views()[0].id.clone();
    actions.invoke_rename_smart(view.clone().into(), "Renamed filter".into());
    assert_eq!(read().smart_views()[0].name, "Renamed filter");
    store.set_filter_tags("rust".into());
    actions.invoke_filter_changed();
    assert_eq!(visible().len(), 2);
    actions.invoke_open_smart(view.clone().into());
    assert!(visible().is_empty());
    actions.invoke_delete_smart(view.into());
    store.set_filter_tags("".into());
    actions.invoke_filter_changed();

    actions.invoke_open_task(template.clone().into());
    actions.invoke_duplicate_task();
    let copy = main.global::<Editor>().get_id().to_string();
    assert_ne!(copy, template);
    actions.invoke_editor_save();
    assert!(read().task(&copy).unwrap().active());
    assert_eq!(visible().len(), 3);
    actions.invoke_browse(1);
    assert_eq!(visible(), [template]);
    actions.invoke_browse(2);
    assert_eq!(visible().as_slice(), std::slice::from_ref(&archived));
    assert!(overlay
        .global::<Store>()
        .get_today_items()
        .iter()
        .all(|t| t.id != archived));
    assert!(overlay
        .global::<Store>()
        .get_board()
        .iter()
        .flat_map(|c| c.cards.iter().collect::<Vec<_>>())
        .all(|t| t.id != archived));
    assert!(host
        .0
        .borrow()
        .iter()
        .all(|r| !r.key.starts_with(&archived)));
    actions.invoke_open_task(archived.clone().into());
    main.global::<Editor>().set_archived(false);
    actions.invoke_editor_save();
    assert!(!read().task(&archived).unwrap().archived);
    actions.invoke_browse(0);
    assert!(visible().contains(&archived));

    // Inspector membership edits and parent choices persist through the ordinary save.
    actions.invoke_open_task(second.clone().into());
    let ed = main.global::<Editor>();
    ed.set_tags("phone rust".into());
    let parent = ed
        .get_parent_options()
        .iter()
        .position(|s| s == "First")
        .unwrap();
    ed.set_parent_index(parent as i32);
    actions.invoke_blocker_toggle(first.clone().into());
    actions.invoke_editor_save();
    let child = read().task(&second).unwrap();
    assert_eq!(child.tags, ["phone", "rust"]);
    assert_eq!(child.parent, Some(first.clone()));
    assert_eq!(child.blockers, [first]);
    actions.invoke_toggle_done(second.into());
    assert!(store.get_toast().contains("blockers"));

    actions.invoke_open_task(child.blockers[0].clone().into());
    let ed = main.global::<Editor>();
    let repeat = ed
        .get_repeats()
        .iter()
        .position(|label| label.contains("after finishing"))
        .unwrap();
    ed.set_repeat_index(repeat as i32);
    actions.invoke_editor_save();
    let key = host.0.borrow()[0].key.clone();
    App::complete_current_task(&key);
    let advanced = read().task(&child.blockers[0]).unwrap().start;
    App::complete_current_task(&key);
    assert_eq!(
        read().task(&child.blockers[0]).unwrap().start,
        advanced,
        "a stale reminder cannot complete the next repeat"
    );
    assert_eq!(App::current_task_open(&key), Some(false));
    app.save_now();
    drop(main);
    drop(overlay);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
