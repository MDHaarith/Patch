use patch_core::doc::{COL_DONE, COL_TODO, DEFAULT_LIST};
use patch_core::model::{Filter, Repeat, SmartView, Unit};
use patch_core::time::parse_dt;
use patch_core::{Doc, Record, Stamp, Task};
use serde_json::json;

fn task(doc: &mut Doc, title: &str) -> String {
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.title = title.into();
    doc.add_task(t)
}

fn sync(a: &mut Doc, b: &mut Doc) {
    let left = a.pending();
    let right = b.pending();
    a.apply_remote(&right);
    b.apply_remote(&left);
    a.ack(&left);
    b.ack(&right);
}

#[test]
fn independent_memberships_and_parent_fields_converge_without_stale_editor_loss() {
    let mut a = Doc::with_node("a");
    let id = task(&mut a, "Task");
    let parent = task(&mut a, "Parent");
    let one = task(&mut a, "First blocker");
    let two = task(&mut a, "Second blocker");
    let mut b = Doc::with_node("b");
    sync(&mut a, &mut b);
    let base = a.task(&id).unwrap();
    let mut left = base.clone();
    left.tags = vec!["rust".into()];
    left.reminders = vec![5];
    left.parent = Some(parent.clone());
    left.blockers = vec![one.clone()];
    a.save_task_from(&base, &left);
    let mut right = base.clone();
    right.tags = vec!["work".into()];
    right.reminders = vec![60];
    right.blockers = vec![two.clone()];
    b.save_task_from(&base, &right);
    sync(&mut a, &mut b);
    let merged = a.task(&id).unwrap();
    assert_eq!(Some(merged.clone()), b.task(&id));
    assert_eq!(merged.tags, ["rust", "work"]);
    assert_eq!(merged.reminder_offsets(), [5, 60]);
    assert_eq!(merged.parent, Some(parent));
    assert!(merged.blockers.contains(&one) && merged.blockers.contains(&two));
    // A stale editor removes only the member it saw, preserving concurrent additions.
    left.tags.clear();
    left.reminders.clear();
    left.blockers.clear();
    a.save_task_from(
        &a.task(&id)
            .map(|_| {
                let mut seen = base.clone();
                seen.tags = vec!["rust".into()];
                seen.reminders = vec![5];
                seen.blockers = vec![one];
                seen
            })
            .unwrap(),
        &left,
    );
    sync(&mut a, &mut b);
    assert_eq!(a.task(&id), b.task(&id));
    assert_eq!(a.task(&id).unwrap().tags, ["work"]);
    assert_eq!(a.task(&id).unwrap().reminder_offsets(), [60]);
    assert_eq!(a.task(&id).unwrap().blockers, [two]);
}

#[test]
fn old_records_default_fields_and_unknown_kinds_survive_round_trip() {
    let mut old = Record::new("old", "task");
    old.put("reminder", json!(15), &Stamp::seed());
    old.put(
        "checklist",
        json!([{"id":"one", "text":"Legacy", "done":true}]),
        &Stamp::seed(),
    );
    let parsed =
        Task::from_record(&serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap())
            .unwrap();
    assert_eq!(parsed.reminder_offsets(), [15]);
    assert!(parsed.tags.is_empty() && parsed.blockers.is_empty() && parsed.parent.is_none());
    assert!(parsed.active());
    assert!(parsed.checklist[0].done);
    let mut future = Record::new("future", "future-record-kind");
    future.put("unknown", json!({"a":[1,2,3]}), &Stamp::seed());
    let mut d = Doc::with_node("reader");
    d.apply_remote(&[old, future.clone()]);
    assert_eq!(d.tasks().len(), 1);
    assert!(d.smart_views().is_empty());
    let restored = Doc::from_json(&d.to_json()).unwrap();
    assert_eq!(restored.record("future"), Some(&future));
    assert!(Task::from_record(&future).is_none());
}

#[test]
fn smart_view_concurrent_name_filter_and_tags_merge() {
    let mut a = Doc::with_node("a");
    let base = SmartView {
        id: "view".into(),
        name: "Work".into(),
        filter: Filter::default(),
    };
    a.save_smart_view_from(&base, &base);
    let mut b = Doc::with_node("b");
    sync(&mut a, &mut b);
    let mut left = base.clone();
    left.name = "Renamed".into();
    left.filter.tags = vec!["rust".into()];
    left.filter.due = "today".into();
    a.save_smart_view_from(&base, &left);
    let mut right = base.clone();
    right.filter.priority = Some(3);
    right.filter.tags = vec!["work".into()];
    right.filter.include_done = true;
    b.save_smart_view_from(&base, &right);
    sync(&mut a, &mut b);
    assert_eq!(a.smart_views(), b.smart_views());
    let view = &a.smart_views()[0];
    assert_eq!(view.name, "Renamed");
    assert_eq!(view.filter.tags, ["rust", "work"]);
    assert_eq!(view.filter.priority, Some(3));
    assert_eq!(view.filter.due, "today");
    assert!(view.filter.include_done);
    a.delete_smart_view("view");
    sync(&mut a, &mut b);
    assert!(b.smart_views().is_empty());
}

#[test]
fn undo_redo_sync_with_new_stamps_preserves_unrelated_remote_edits() {
    let mut a = Doc::with_node("a");
    let id = task(&mut a, "Before");
    let mut b = Doc::with_node("b");
    sync(&mut a, &mut b);
    let before = a.clone();
    let mut t = a.task(&id).unwrap();
    t.title = "After".into();
    t.tags = vec!["rust".into()];
    a.save_task(&t);
    a.move_task(&id, COL_DONE, 0);
    let changes = a.changes_since(&before);
    let stamp = a.record(&id).unwrap().f["title"].t.clone();
    sync(&mut a, &mut b);
    let mut t = b.task(&id).unwrap();
    t.notes = "Remote notes".into();
    b.save_task(&t);
    sync(&mut a, &mut b);
    a.replay(&changes, true);
    assert!(a.record(&id).unwrap().f["title"].t > stamp);
    sync(&mut a, &mut b);
    let t = a.task(&id).unwrap();
    assert_eq!(Some(t.clone()), b.task(&id));
    assert_eq!(t.title, "Before");
    assert_eq!(t.notes, "Remote notes");
    assert!(!t.done && t.tags.is_empty());
    a.replay(&changes, false);
    sync(&mut a, &mut b);
    let t = a.task(&id).unwrap();
    assert_eq!(Some(t.clone()), b.task(&id));
    assert_eq!(t.title, "After");
    assert!(t.done);
    assert_eq!(t.notes, "Remote notes");
}

#[test]
fn undo_creation_list_column_deletion_and_bulk_are_single_changesets() {
    let mut d = Doc::with_node("a");
    let before = d.clone();
    let id = task(&mut d, "Created");
    let creation = d.changes_since(&before);
    d.replay(&creation, true);
    assert!(d.task(&id).is_none());
    d.replay(&creation, false);
    assert_eq!(d.task(&id).unwrap().title, "Created");
    let list = d.add_list("Extra", "#ffffff");
    let col = d.add_column("Review");
    let mut t = d.task(&id).unwrap();
    t.list = list.clone();
    t.column = col.clone();
    d.save_task(&t);
    let before = d.clone();
    d.delete_list(&list);
    d.delete_column(&col);
    d.delete_task(&id);
    let bulk = d.changes_since(&before);
    d.replay(&bulk, true);
    assert_eq!(d.task(&id), before.task(&id));
    assert_eq!(d.list(&list), before.list(&list));
    assert_eq!(d.column(&col), before.column(&col));
}

#[test]
fn completion_relative_repeats_use_finish_time_and_do_not_expand_future_dates() {
    for (rule, expected) in [
        (Repeat::AfterDone(3, Unit::Day), "2026-02-03T16:20"),
        (Repeat::AfterDone(1, Unit::Month), "2026-02-28T16:20"),
        (Repeat::AfterDone(2, Unit::Week), "2026-02-14T16:20"),
    ] {
        let mut d = Doc::with_node("a");
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.start = parse_dt("2026-01-01T09:00");
        t.end = parse_dt("2026-01-01T10:00");
        t.repeat = rule;
        let id = d.add_task(t);
        let completed = parse_dt("2026-01-31T16:20").unwrap();
        d.set_done_at(&id, true, completed);
        let t = d.task(&id).unwrap();
        assert_eq!(t.start, parse_dt(expected));
        assert_eq!(t.duration(), chrono::Duration::hours(1));
        assert!(!t.done);
        let occurrences =
            patch_core::recur::occurrences(&t, completed, parse_dt("2027-01-01").unwrap());
        assert_eq!(occurrences.len(), 1);
        assert_eq!(Repeat::parse(&rule.encode()), rule);
    }
    let q = patch_core::quick::parse(
        "Water every 3 days after done #garden",
        parse_dt("2026-01-31").unwrap(),
        &[],
    );
    assert_eq!(q.title, "Water");
    assert_eq!(q.repeat, Repeat::AfterDone(3, Unit::Day));
    assert_eq!(q.tags, ["garden"]);
}

#[test]
fn local_cycles_rejected_concurrent_cycles_stored_and_missing_edges_safe() {
    let mut a = Doc::with_node("a");
    let one = task(&mut a, "One");
    let two = task(&mut a, "Two");
    let mut b = Doc::with_node("b");
    sync(&mut a, &mut b);
    let mut left = a.task(&one).unwrap();
    left.parent = Some(two.clone());
    left.blockers = vec![two.clone()];
    a.save_task(&left);
    let mut right = b.task(&two).unwrap();
    right.parent = Some(one.clone());
    right.blockers = vec![one.clone()];
    b.save_task(&right);
    // The same cycle is rejected when the second edge is known locally.
    assert!(!a.links_valid(&right));
    let before = a.task(&two);
    a.save_task(&right);
    assert_eq!(a.task(&two), before);
    sync(&mut a, &mut b);
    assert_eq!(a.tasks(), b.tasks());
    assert_eq!(a.task(&one).unwrap().parent, Some(two.clone()));
    assert_eq!(a.task(&two).unwrap().parent, Some(one.clone()));
    assert!(a.blocked(&left));
    a.delete_task(&two);
    assert!(!a.blocked(&left));
    left.notes = "Unrelated edits still work".into();
    a.save_task(&left);
    assert_eq!(a.task(&one).unwrap().notes, left.notes);
}

#[test]
fn template_duplicate_and_archive_preserve_source_and_checklist() {
    let mut d = Doc::with_node("a");
    let mut template = Task::blank("", DEFAULT_LIST, COL_TODO);
    template.template = true;
    template.start = parse_dt("2026-11-04T09:00");
    template.checklist = vec![patch_core::model::CheckItem {
        id: "check".into(),
        text: "Step".into(),
        done: true,
    }];
    let id = d.add_task(template);
    let copy = d.duplicate_task(&id).unwrap();
    let t = d.task(&copy).unwrap();
    assert!(t.active() && t.start.is_none());
    assert!(!t.checklist[0].done);
    assert_ne!(t.checklist[0].id, "check");
    assert!(d.task(&id).unwrap().checklist[0].done);
    d.set_done(&copy, true);
    d.auto_archive(7, patch_core::hlc::now_ms() as i64 + 8 * 86_400_000);
    assert!(d.task(&copy).unwrap().archived);
    assert!(!d.task(&id).unwrap().archived);
    assert!(d.record(&copy).is_some_and(|r| !r.is_deleted()));
}

#[test]
fn due_filters_use_exclusive_ends_and_expand_calendar_repeats() {
    let mut task = Task::blank("filter", DEFAULT_LIST, COL_TODO);
    task.start = parse_dt("2026-11-03T23:00");
    task.end = parse_dt("2026-11-04T00:00");
    let now = parse_dt("2026-11-04T10:00").unwrap();
    let mut filter = Filter {
        due: "today".into(),
        ..Filter::default()
    };
    assert!(!filter.matches(&task, now));
    task.repeat = Repeat::Daily;
    assert!(filter.matches(&task, now));
    task.repeat = Repeat::None;
    task.start = parse_dt("2026-11-06T09:00");
    task.end = None;
    filter.due = "week".into();
    assert!(filter.matches(&task, now));
    task.start = parse_dt("2026-11-09T09:00");
    assert!(!filter.matches(&task, now));
}

#[test]
fn quick_add_only_exact_list_names_select_a_list() {
    let doc = Doc::default();
    let q = patch_core::quick::parse(
        "Review #work #wo #rust",
        parse_dt("2026-11-04T10:00").unwrap(),
        &doc.lists(),
    );
    assert_eq!(q.list.as_deref(), Some(patch_core::doc::LIST_WORK));
    assert_eq!(q.tags, ["wo", "rust"]);
    assert_eq!(q.title, "Review");
}

#[test]
fn all_day_after_completion_stays_at_midnight() {
    let mut doc = Doc::default();
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.start = parse_dt("2026-11-01");
    t.all_day = true;
    t.repeat = Repeat::AfterDone(3, Unit::Day);
    let id = doc.add_task(t);
    doc.set_done_at(&id, true, parse_dt("2026-11-05T18:30").unwrap());
    let t = doc.task(&id).unwrap();
    assert_eq!(t.start, parse_dt("2026-11-08"));
    assert_eq!(t.duration(), chrono::Duration::days(1));
}

#[test]
fn inspector_status_completion_advances_repeat_and_reopening_clears_done_time() {
    let mut doc = Doc::default();
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.start = parse_dt("2026-11-05T09:00");
    t.repeat = Repeat::Daily;
    let id = doc.add_task(t);
    let base = doc.task(&id).unwrap();
    let mut draft = base.clone();
    draft.column = COL_DONE.into();
    draft.notes = "Inspector edit".into();
    doc.save_task_from(&base, &draft);
    let t = doc.task(&id).unwrap();
    assert!(!t.done);
    assert_eq!(t.start, parse_dt("2026-11-06T09:00"));
    assert_eq!(t.notes, "Inspector edit");
    let mut plain = t;
    plain.repeat = Repeat::None;
    doc.save_task(&plain);
    doc.set_done(&id, true);
    let base = doc.task(&id).unwrap();
    let mut draft = base.clone();
    draft.column = COL_TODO.into();
    doc.save_task_from(&base, &draft);
    assert_eq!(doc.task(&id).unwrap().done_at, None);
}

fn repeating(doc: &mut Doc, title: &str, start: &str, repeat: Repeat) -> String {
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.title = title.into();
    t.start = parse_dt(start);
    t.repeat = repeat;
    doc.add_task(t)
}

#[test]
fn skipping_rolls_forward_without_counting_as_done() {
    let mut doc = Doc::with_node("a");
    let id = repeating(&mut doc, "Standup", "2026-11-02T09:00", Repeat::Daily);
    let now = parse_dt("2026-11-02T08:00").unwrap();
    assert!(doc.skip_occurrence(&id, now));
    assert_eq!(doc.task(&id).unwrap().start, parse_dt("2026-11-03T09:00"));
    assert_eq!(doc.stats(now.date()).today, 0);
    let plain = task(&mut doc, "One-off");
    assert!(!doc.skip_occurrence(&plain, now));
}

#[test]
fn streaks_count_repeats_survive_deletion_and_undo() {
    let mut doc = Doc::with_node("a");
    let id = repeating(&mut doc, "Gym", "2026-11-01T09:00", Repeat::Daily);
    for day in ["2026-11-01", "2026-11-02", "2026-11-04", "2026-11-05"] {
        doc.set_done_at(&id, true, parse_dt(&format!("{day}T10:00")).unwrap());
    }
    let today = parse_dt("2026-11-05T12:00").unwrap().date();
    let stats = doc.stats(today);
    assert_eq!((stats.today, stats.streak, stats.best), (1, 2, 2));
    assert_eq!(stats.last_7, [0, 0, 1, 1, 0, 1, 1]);
    // Today not done yet doesn't end the streak; a whole missed day does.
    assert_eq!(doc.stats(today + chrono::Duration::days(1)).streak, 2);
    assert_eq!(doc.stats(today + chrono::Duration::days(2)).streak, 0);
    doc.delete_task(&id);
    assert_eq!(doc.stats(today).streak, 2);

    let mut doc = Doc::with_node("b");
    let id = repeating(&mut doc, "Gym", "2026-11-05T09:00", Repeat::Daily);
    let before = doc.clone();
    doc.set_done_at(&id, true, parse_dt("2026-11-05T10:00").unwrap());
    let change = doc.changes_since(&before);
    assert_eq!(doc.stats(today).today, 1);
    doc.replay(&change, true);
    assert_eq!(doc.stats(today).today, 0, "undo removes the completion from the stats");
}

#[test]
fn overdue_tasks_are_brought_up_to_date() {
    let mut doc = Doc::with_node("a");
    let now = parse_dt("2026-11-05T14:00").unwrap();
    let later = repeating(&mut doc, "Later today", "2026-11-02T18:00", Repeat::None);
    let passed = repeating(&mut doc, "Time passed", "2026-11-02T09:00", Repeat::None);
    let daily = repeating(&mut doc, "Standup", "2026-10-01T09:30", Repeat::Daily);
    let after = repeating(&mut doc, "Water plants", "2026-11-01T09:00", Repeat::AfterDone(3, Unit::Day));
    let future = repeating(&mut doc, "Next week", "2026-11-12T09:00", Repeat::None);
    let mut archived = doc.task(&passed).unwrap();
    archived.id = String::new();
    archived.archived = true;
    let archived = doc.add_task(archived);

    assert_eq!(doc.reschedule_overdue(now), 4);
    assert_eq!(doc.task(&later).unwrap().start, parse_dt("2026-11-05T18:00"));
    let moved = doc.task(&passed).unwrap();
    assert!(moved.all_day && moved.start == parse_dt("2026-11-05T00:00") && !moved.is_overdue(now));
    assert_eq!(doc.task(&daily).unwrap().start, parse_dt("2026-11-06T09:30"));
    assert!(!doc.task(&after).unwrap().is_overdue(now));
    assert_eq!(doc.task(&future).unwrap().start, parse_dt("2026-11-12T09:00"));
    assert_eq!(doc.task(&archived).unwrap().start, parse_dt("2026-11-02T09:00"));
    assert_eq!(doc.reschedule_overdue(now), 0);
}

#[test]
fn focus_minutes_add_up_per_day() {
    let mut doc = Doc::with_node("a");
    let id = task(&mut doc, "Report");
    let day = parse_dt("2026-11-05T12:00").unwrap().date();
    doc.log_focus(&id, 25, day);
    doc.log_focus(&id, 15, day);
    doc.log_focus(&id, 50, day - chrono::Duration::days(1));
    assert_eq!(doc.stats(day).focus_today, 40);
}

#[test]
fn note_links_are_http_only_and_trimmed() {
    let notes = "See (https://a.example/x), http://b.example. javascript:alert(1) file:///etc/passwd https:// ftp://c";
    assert_eq!(patch_core::model::links_in(notes), ["https://a.example/x", "http://b.example"]);
}
