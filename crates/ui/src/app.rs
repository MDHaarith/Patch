//! The application controller: owns the document, the sync client and the
//! UI state, and keeps every attached window's view models up to date.

use crate::models::{self, color, Ctx};
use crate::prefs::Prefs;
use crate::{
    Actions, CheckRow, Choice, DayCell, Editor, HostedHub, Look, MainWindow, OverlayWindow, Store, Theme,
};
use crate::{Bar, Block, BoardCol, DayHead, ListInfo, Section, TaskItem};
use crate::Host;
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::model::{CheckItem, Repeat, Task, PALETTE};
use patch_core::time::{add_months, fmt_hour_label, hm, minutes_of, month_start, now_local};
use patch_core::{quick, Doc};
use patch_sync::{SyncClient, SyncConfig, SyncEvent, SyncStatus};
use slint::{ComponentHandle, Model, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

const REMINDER_CHOICES: [Option<i64>; 6] = [None, Some(0), Some(5), Some(15), Some(60), Some(1440)];

thread_local! {
    // Lets work posted from the sync thread find the App on the UI thread.
    static CURRENT: RefCell<Weak<App>> = const { RefCell::new(Weak::new()) };
}

/// Shared view models; the same `Rc`s are handed to every window so they
/// stay in sync without copying.
struct Models {
    mini: Rc<VecModel<DayCell>>,
    month: Rc<VecModel<DayCell>>,
    agenda: Rc<VecModel<TaskItem>>,
    days: Rc<VecModel<DayHead>>,
    blocks: Rc<VecModel<Block>>,
    bars: Rc<VecModel<Bar>>,
    board: Rc<VecModel<BoardCol>>,
    sections: Rc<VecModel<Section>>,
    lists: Rc<VecModel<ListInfo>>,
    up_next: Rc<VecModel<TaskItem>>,
    today: Rc<VecModel<TaskItem>>,
    results: Rc<VecModel<TaskItem>>,
    hours: Rc<VecModel<SharedString>>,
    weekdays: Rc<VecModel<SharedString>>,
    weekdays_short: Rc<VecModel<SharedString>>,
    picker: Rc<VecModel<DayCell>>,
    checklist: Rc<VecModel<CheckRow>>,
    editor_lists: Rc<VecModel<Choice>>,
    editor_columns: Rc<VecModel<Choice>>,
}

impl Models {
    fn new() -> Models {
        Models {
            mini: Rc::default(),
            month: Rc::default(),
            agenda: Rc::default(),
            days: Rc::default(),
            blocks: Rc::default(),
            bars: Rc::default(),
            board: Rc::default(),
            sections: Rc::default(),
            lists: Rc::default(),
            up_next: Rc::default(),
            today: Rc::default(),
            results: Rc::default(),
            hours: Rc::default(),
            weekdays: Rc::default(),
            weekdays_short: Rc::default(),
            picker: Rc::default(),
            checklist: Rc::default(),
            editor_lists: Rc::default(),
            editor_columns: Rc::default(),
        }
    }

    fn bind(&self, s: &Store) {
        s.set_mini_cells(self.mini.clone().into());
        s.set_month_cells(self.month.clone().into());
        s.set_day_agenda(self.agenda.clone().into());
        s.set_grid_days(self.days.clone().into());
        s.set_grid_blocks(self.blocks.clone().into());
        s.set_grid_bars(self.bars.clone().into());
        s.set_board(self.board.clone().into());
        s.set_sections(self.sections.clone().into());
        s.set_lists(self.lists.clone().into());
        s.set_up_next(self.up_next.clone().into());
        s.set_today_items(self.today.clone().into());
        s.set_results(self.results.clone().into());
        s.set_hour_labels(self.hours.clone().into());
        s.set_weekdays(self.weekdays.clone().into());
        s.set_weekdays_short(self.weekdays_short.clone().into());
    }

    fn bind_editor(&self, e: &Editor) {
        e.set_picker_cells(self.picker.clone().into());
        e.set_checklist(self.checklist.clone().into());
        e.set_lists(self.editor_lists.clone().into());
        e.set_columns(self.editor_columns.clone().into());
    }
}

/// Updates a model in place so unchanged rows keep their UI state (hover,
/// in-flight drags, animations) across refreshes.
fn update<T: Clone + PartialEq + 'static>(m: &VecModel<T>, rows: Vec<T>) {
    let old = m.row_count();
    for (i, row) in rows.iter().enumerate() {
        if i < old {
            if m.row_data(i).as_ref() != Some(row) {
                m.set_row_data(i, row.clone());
            }
        } else {
            m.push(row.clone());
        }
    }
    for i in (rows.len()..old).rev() {
        m.remove(i);
    }
}

struct EditorState {
    task: Task,
    is_new: bool,
    /// Next tap in the range picker sets the end date.
    picking_end: bool,
    picker_month: NaiveDate,
}

struct UiState {
    view: i32,
    anchor: NaiveDate,
    selected: NaiveDate,
    mini_month: NaiveDate,
    search: String,
    editor: Option<EditorState>,
    today: NaiveDate,
}

pub struct App {
    data_dir: PathBuf,
    host: Rc<dyn Host>,
    doc: Arc<Mutex<Doc>>,
    sync: SyncClient,
    prefs: RefCell<Prefs>,
    state: RefCell<UiState>,
    models: Models,
    main: RefCell<Option<slint::Weak<MainWindow>>>,
    overlay: RefCell<Option<slint::Weak<OverlayWindow>>>,
    status: RefCell<SyncStatus>,
    hosted: RefCell<Option<HostedHub>>,
    saved_rev: Cell<u64>,
    focus_count: Cell<i32>,
    /// The user switched the overlay on but Android still lacked permission.
    overlay_pending: Cell<bool>,
    timers: RefCell<Vec<Timer>>,
    toast_timer: Timer,
    focus_timer: Timer,
    grain: slint::Image,
    icon: slint::Image,
}

fn grain_image() -> slint::Image {
    // Fine film grain for the matte finish; deterministic so it never shimmers.
    const N: u32 = 160;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(N, N);
    let mut seed: u32 = 0x9e37_79b9;
    for px in buf.make_mut_slice() {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let light = seed & 1 == 0;
        let a = ((seed >> 8) % if light { 9 } else { 14 }) as u8;
        let v = if light { a } else { 0 };
        *px = slint::Rgba8Pixel { r: v, g: v, b: v, a };
    }
    slint::Image::from_rgba8_premultiplied(buf)
}

fn icon_image() -> slint::Image {
    static PNG: &[u8] = include_bytes!("../../../assets/images/icon.png");
    let decode = || -> Option<slint::Image> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(PNG));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut data = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut data).ok()?;
        if info.color_type != png::ColorType::Rgba {
            return None;
        }
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&data[..info.buffer_size()], info.width, info.height);
        Some(slint::Image::from_rgba8(buf))
    };
    decode().unwrap_or_default()
}

fn parse_key(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

impl App {
    /// Loads the document and preferences from `data_dir` and starts syncing.
    pub fn new(data_dir: PathBuf, host: Rc<dyn Host>) -> Rc<App> {
        let _ = std::fs::create_dir_all(&data_dir);
        let doc = Arc::new(Mutex::new(Doc::load_or_new(&data_dir.join("doc.json"))));
        let prefs = Prefs::load(&data_dir.join("prefs.json"));
        let sync = SyncClient::start(doc.clone(), None, |ev| {
            let _ = slint::invoke_from_event_loop(move || {
                CURRENT.with(|c| {
                    if let Some(app) = c.borrow().upgrade() {
                        app.on_sync(ev);
                    }
                })
            });
        });
        let today = now_local().date();
        let mobile = host.is_mobile();
        let view = if mobile && prefs.view == 1 { 2 } else { prefs.view.clamp(0, 4) };
        let app = Rc::new(App {
            data_dir,
            doc,
            sync,
            state: RefCell::new(UiState {
                view,
                anchor: today,
                selected: today,
                mini_month: month_start(today),
                search: String::new(),
                editor: None,
                today,
            }),
            prefs: RefCell::new(prefs),
            models: Models::new(),
            main: RefCell::new(None),
            overlay: RefCell::new(None),
            status: RefCell::new(SyncStatus::Disabled),
            hosted: RefCell::new(None),
            saved_rev: Cell::new(0),
            focus_count: Cell::new(0),
            overlay_pending: Cell::new(false),
            timers: RefCell::new(Vec::new()),
            toast_timer: Timer::default(),
            focus_timer: Timer::default(),
            grain: grain_image(),
            icon: icon_image(),
            host,
        });
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&app));
        app.saved_rev.set(app.doc.lock().unwrap().rev());
        app.configure_sync();
        app.start_timers();
        app.refresh();
        app
    }

    fn configure_sync(&self) {
        let prefs = self.prefs.borrow().clone();
        if prefs.hosting && self.host.can_host_hub() {
            match self.host.set_hosting(true) {
                Ok(Some(hub)) => {
                    self.sync.set_config(SyncConfig::new(&hub.local_url, &hub.token).ok());
                    *self.hosted.borrow_mut() = Some(hub);
                    return;
                }
                Ok(None) => {}
                Err(e) => self.toast(&format!("Couldn't start the hub: {e}")),
            }
        }
        let cfg = if prefs.hub_url.is_empty() { None } else { SyncConfig::new(&prefs.hub_url, &prefs.hub_token).ok() };
        self.sync.set_config(cfg);
    }

    fn start_timers(self: &Rc<Self>) {
        let mut timers = self.timers.borrow_mut();
        // Persist edits shortly after they happen.
        let weak = Rc::downgrade(self);
        let t = Timer::default();
        t.start(TimerMode::Repeated, StdDuration::from_millis(1500), move || {
            if let Some(app) = weak.upgrade() {
                app.save_if_dirty();
            }
        });
        timers.push(t);
        // Clock, "now" line, day rollover and reminders.
        let weak = Rc::downgrade(self);
        let t = Timer::default();
        t.start(TimerMode::Repeated, StdDuration::from_secs(15), move || {
            if let Some(app) = weak.upgrade() {
                app.tick();
            }
        });
        timers.push(t);
    }

    fn save_if_dirty(&self) {
        let (rev, json) = {
            let d = self.doc.lock().unwrap();
            if d.rev() == self.saved_rev.get() {
                return;
            }
            (d.rev(), d.to_json())
        };
        if patch_core::doc::write_atomic(&self.data_dir.join("doc.json"), json.as_bytes()).is_ok() {
            self.saved_rev.set(rev);
        }
    }

    fn save_prefs(&self) {
        let _ = self.prefs.borrow().save(&self.data_dir.join("prefs.json"));
    }

    /// Flush the document and prefs to disk (call on pause / quit).
    pub fn save_now(&self) {
        self.save_if_dirty();
        self.save_prefs();
    }

    pub fn prefs(&self) -> Prefs {
        self.prefs.borrow().clone()
    }

    // ------------------------------------------------------------------ windows

    fn each_store(&self, f: impl Fn(&Store)) {
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(&w.global::<Store>());
        }
        if let Some(w) = self.overlay.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(&w.global::<Store>());
        }
    }

    fn each_theme(&self, f: impl Fn(&Theme)) {
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(&w.global::<Theme>());
        }
        if let Some(w) = self.overlay.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(&w.global::<Theme>());
        }
    }

    fn with_editor(&self, f: impl Fn(&Editor)) {
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(&w.global::<Editor>());
        }
    }

    fn init_store(&self, s: &Store, theme: &Theme) {
        self.models.bind(s);
        let mobile = self.host.is_mobile();
        s.set_mobile(mobile);
        s.set_can_host(self.host.can_host_hub());
        s.set_version(env!("CARGO_PKG_VERSION").into());
        s.set_grain(self.grain.clone());
        s.set_app_icon(self.icon.clone());
        let p = self.prefs.borrow();
        s.set_sidebar_open(p.sidebar);
        s.set_overlay_expanded(p.overlay_expanded);
        theme.set_compact(mobile);
        drop(p);
        self.apply_prefs_to(s, theme);
    }

    fn apply_prefs_to(&self, s: &Store, theme: &Theme) {
        let p = self.prefs.borrow();
        theme.set_look(match p.look {
            1 => Look::Glass,
            2 => Look::Light,
            _ => Look::Matte,
        });
        s.set_week_monday(p.week_monday);
        s.set_time_24h(p.time_24h);
        s.set_autostart(p.autostart);
        s.set_widget_on(p.widget);
        s.set_overlay_on(p.overlay);
        s.set_overlay_boot(p.overlay_boot);
        s.set_overlay_allowed(self.host.overlay_allowed());
        s.set_hub_url(p.hub_url.clone().into());
        s.set_hub_token(p.hub_token.clone().into());
        let hosted = self.hosted.borrow();
        s.set_hosting(hosted.is_some());
        if let Some(h) = hosted.as_ref() {
            s.set_host_urls(h.urls.join("\n").into());
            s.set_host_token(h.token.clone().into());
        }
        self.apply_status_to(s);
    }

    /// Wire up the main window (full calendar/board UI).
    pub fn attach_main(self: &Rc<Self>, w: &MainWindow) {
        *self.main.borrow_mut() = Some(w.as_weak());
        self.init_store(&w.global::<Store>(), &w.global::<Theme>());
        self.models.bind_editor(&w.global::<Editor>());
        self.wire(&w.global::<Actions>());
        self.refresh();
    }

    /// Wire up the floating overlay (Android) / widget (Linux) window.
    pub fn attach_overlay(self: &Rc<Self>, w: &OverlayWindow) {
        *self.overlay.borrow_mut() = Some(w.as_weak());
        self.init_store(&w.global::<Store>(), &w.global::<Theme>());
        self.wire(&w.global::<Actions>());
        self.refresh();
    }

    pub fn detach_main(&self) {
        *self.main.borrow_mut() = None;
    }

    pub fn detach_overlay(&self) {
        *self.overlay.borrow_mut() = None;
    }

    /// Insets in logical pixels (status bar / gesture bar on Android).
    pub fn set_safe_area(&self, top: f32, bottom: f32) {
        self.each_store(|s| {
            s.set_safe_top(top);
            s.set_safe_bottom(bottom);
        });
    }

    pub fn set_overlay_allowed(&self, allowed: bool) {
        self.each_store(|s| s.set_overlay_allowed(allowed));
        if allowed && self.overlay_pending.replace(false) && self.host.set_overlay(true) {
            self.prefs.borrow_mut().overlay = true;
            self.save_prefs();
            self.each_store(|s| s.set_overlay_on(true));
            return;
        }
        if !allowed && self.prefs.borrow().overlay {
            self.prefs.borrow_mut().overlay = false;
            self.save_prefs();
            self.each_store(|s| s.set_overlay_on(false));
        }
    }

    pub fn overlay_expanded(&self) -> bool {
        self.prefs.borrow().overlay_expanded
    }

    /// Android back button. Returns true if the UI consumed it.
    pub fn back(&self) -> bool {
        let mut consumed = false;
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            let s = w.global::<Store>();
            if s.get_settings_open() {
                s.set_settings_open(false);
                consumed = true;
            } else if s.get_search_open() {
                s.set_search_open(false);
                consumed = true;
            }
        }
        if !consumed && self.state.borrow().editor.is_some() {
            self.close_editor(true);
            consumed = true;
        }
        consumed
    }

    // ------------------------------------------------------------------ actions

    fn wire(self: &Rc<Self>, a: &Actions) {
        macro_rules! on {
            ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = Rc::downgrade(self);
                a.$setter(move |$($arg),*| {
                    if let Some($app) = weak.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_nav, |app, delta| app.nav(delta));
        on!(on_go_today, |app| app.go_today());
        on!(on_set_view, |app, v| app.set_view(v));
        on!(on_select_date, |app, key| app.select_date(&key));
        on!(on_mini_nav, |app, delta| {
            {
                let mut st = app.state.borrow_mut();
                st.mini_month = add_months(st.mini_month, delta);
            }
            app.refresh();
        });
        on!(on_open_task, |app, id| app.open_task(&id));
        on!(on_new_task, |app, key, minutes| app.new_task(&key, minutes));
        on!(on_toggle_done, |app, id| {
            let done = app.doc.lock().unwrap().task(&id).is_some_and(|t| !t.done);
            app.edit(|d| d.set_done(&id, done));
        });
        on!(on_quick_add, |app, text| app.quick_add(&text, None, false));
        on!(on_quick_add_today, |app, text| app.quick_add(&text, None, true));
        on!(on_quick_add_card, |app, col, text| app.quick_add(&text, Some(col.to_string()), false));
        on!(on_delete_task, |app, id| app.edit(|d| d.delete_task(&id)));
        on!(on_drop_on_date, |app, id, key| app.drop_on_date(&id, &key));
        on!(on_move_block, |app, id, days, minutes| app.move_block(&id, days, minutes));
        on!(on_resize_block, |app, id, minutes| app.resize_block(&id, minutes));
        on!(on_drop_card, |app, id, col, index| app.drop_card(&id, col, index));
        on!(on_add_column, |app, name| app.edit(|d| {
            d.add_column(name.trim());
        }));
        on!(on_rename_column, |app, id, name| {
            let name = name.trim().to_string();
            if !name.is_empty() {
                app.edit(|d| {
                    if let Some(mut c) = d.column(&id) {
                        c.name = name.clone();
                        d.save_column(&c);
                    }
                });
            }
        });
        on!(on_delete_column, |app, id| {
            let ok = app.doc.lock().unwrap().columns().len() > 1;
            if ok {
                app.edit(|d| {
                    d.delete_column(&id);
                });
            } else {
                app.toast("A board needs at least one column");
            }
        });
        on!(on_toggle_list, |app, id| {
            {
                let mut p = app.prefs.borrow_mut();
                let id = id.to_string();
                if let Some(i) = p.hidden_lists.iter().position(|h| *h == id) {
                    p.hidden_lists.remove(i);
                } else {
                    p.hidden_lists.push(id);
                }
            }
            app.save_prefs();
            app.refresh();
        });
        on!(on_add_list, |app, name| {
            let n = app.doc.lock().unwrap().lists().len();
            let (_, hex) = PALETTE[(n * 3 + 5) % PALETTE.len()];
            app.edit(|d| {
                d.add_list(name.trim(), hex);
            });
        });
        on!(on_rename_list, |app, id, name| {
            let name = name.trim().to_string();
            if !name.is_empty() {
                app.edit(|d| {
                    if let Some(mut l) = d.list(&id) {
                        l.name = name.clone();
                        d.save_list(&l);
                    }
                });
            }
        });
        on!(on_recolor_list, |app, id| app.edit(|d| {
            if let Some(mut l) = d.list(&id) {
                let i = PALETTE.iter().position(|(_, h)| h.eq_ignore_ascii_case(&l.color)).map(|i| i + 1).unwrap_or(0);
                l.color = PALETTE[i % PALETTE.len()].1.to_string();
                d.save_list(&l);
            }
        }));
        on!(on_delete_list, |app, id| app.edit(|d| {
            d.delete_list(&id);
        }));
        on!(on_editor_save, |app| app.close_editor(true));
        on!(on_editor_close, |app| app.close_editor(true));
        on!(on_editor_delete, |app| {
            let id = app.state.borrow().editor.as_ref().map(|e| (e.task.id.clone(), e.is_new));
            if let Some((id, is_new)) = id {
                if !is_new {
                    app.edit(|d| d.delete_task(&id));
                    app.toast("Task deleted");
                }
            }
            app.close_editor(false);
        });
        on!(on_picker_nav, |app, delta| {
            if let Some(e) = app.state.borrow_mut().editor.as_mut() {
                e.picker_month = add_months(e.picker_month, delta);
            }
            app.refresh_editor_dates();
        });
        on!(on_picker_tap, |app, key| app.picker_tap(&key));
        on!(on_picker_reset, |app| app.picker_reset());
        on!(on_editor_changed, |app| app.editor_changed());
        on!(on_check_add, |app, text| app.edit_checklist(|c| {
            c.push(CheckItem { id: patch_core::id::new_id("ci"), text: text.trim().to_string(), done: false })
        }));
        on!(on_check_toggle, |app, i| app.edit_checklist(|c| {
            if let Some(item) = c.get_mut(i as usize) {
                item.done = !item.done;
            }
        }));
        on!(on_check_remove, |app, i| app.edit_checklist(|c| {
            if (i as usize) < c.len() {
                c.remove(i as usize);
            }
        }));
        on!(on_search, |app, q| {
            app.state.borrow_mut().search = q.to_string();
            app.refresh();
        });
        on!(on_set_look, |app, look| {
            app.prefs.borrow_mut().look = look;
            app.save_prefs();
            app.each_theme(|t| {
                t.set_look(match look {
                    1 => Look::Glass,
                    2 => Look::Light,
                    _ => Look::Matte,
                })
            });
            app.refresh();
        });
        on!(on_set_week_monday, |app, v| {
            app.prefs.borrow_mut().week_monday = v;
            app.save_prefs();
            app.each_store(|s| s.set_week_monday(v));
            app.refresh();
        });
        on!(on_set_time_24h, |app, v| {
            app.prefs.borrow_mut().time_24h = v;
            app.save_prefs();
            app.each_store(|s| s.set_time_24h(v));
            app.refresh();
        });
        on!(on_connect_hub, |app, url, token| app.connect_hub(&url, &token));
        on!(on_disconnect_hub, |app| {
            {
                let mut p = app.prefs.borrow_mut();
                p.hub_url.clear();
                p.hub_token.clear();
            }
            app.save_prefs();
            app.configure_sync();
            app.each_store(|s| {
                s.set_hub_url("".into());
                s.set_hub_token("".into());
            });
        });
        on!(on_set_hosting, |app, on| app.set_hosting(on));
        on!(on_set_autostart, |app, on| match app.host.set_autostart(on) {
            Ok(()) => {
                app.prefs.borrow_mut().autostart = on;
                app.save_prefs();
            }
            Err(e) => {
                app.toast(&e);
                app.each_store(|s| s.set_autostart(!on));
            }
        });
        on!(on_set_widget, |app, on| {
            app.prefs.borrow_mut().widget = on;
            app.save_prefs();
            app.each_store(|s| s.set_widget_on(on));
            app.host.set_widget(on);
        });
        on!(on_set_overlay, |app, on| {
            let ok = app.host.set_overlay(on);
            app.overlay_pending.set(on && !ok);
            let on = on && ok;
            app.prefs.borrow_mut().overlay = on;
            app.save_prefs();
            app.each_store(|s| {
                s.set_overlay_on(on);
                s.set_overlay_allowed(app.host.overlay_allowed());
            });
        });
        on!(on_set_overlay_boot, |app, on| {
            app.prefs.borrow_mut().overlay_boot = on;
            app.save_prefs();
            app.host.set_overlay_boot(on);
        });
        on!(on_copy_text, |app, text| {
            app.host.copy_text(&text);
            app.toast("Copied");
        });
        on!(on_overlay_expand, |app, expanded| {
            app.prefs.borrow_mut().overlay_expanded = expanded;
            app.save_prefs();
            app.each_store(|s| s.set_overlay_expanded(expanded));
            app.host.overlay_expand(expanded);
        });
        on!(on_open_app, |app, id| {
            let id = id.to_string();
            app.host.open_app((!id.is_empty()).then_some(id.as_str()));
            if !id.is_empty() {
                app.open_task(&id);
            }
        });
        on!(on_text_focus, |app, focused| app.text_focus(focused));
    }

    /// Runs a document mutation, then refreshes the UI and nudges sync.
    fn edit(&self, f: impl FnOnce(&mut Doc)) {
        {
            let mut d = self.doc.lock().unwrap();
            f(&mut d);
        }
        self.sync.notify_local_change();
        self.refresh();
    }

    fn toast(&self, text: &str) {
        self.each_store(|s| s.set_toast(text.into()));
        let weak = CURRENT.with(|c| c.borrow().clone());
        self.toast_timer.start(TimerMode::SingleShot, StdDuration::from_millis(2400), move || {
            if let Some(app) = weak.upgrade() {
                app.each_store(|s| s.set_toast("".into()));
            }
        });
    }

    fn text_focus(&self, focused: bool) {
        // Focus moves out of one field before it enters the next; settle
        // briefly so the keyboard doesn't flicker and shortcuts come back
        // only when no field holds focus.
        self.focus_count.set((self.focus_count.get() + if focused { 1 } else { -1 }).max(0));
        let weak = CURRENT.with(|c| c.borrow().clone());
        self.focus_timer.start(TimerMode::SingleShot, StdDuration::from_millis(60), move || {
            if let Some(app) = weak.upgrade() {
                let any = app.focus_count.get() > 0;
                app.host.text_focus(any);
                if !any {
                    if let Some(w) = app.main.borrow().as_ref().and_then(|w| w.upgrade()) {
                        w.invoke_refocus();
                    }
                }
            }
        });
    }

    fn nav(&self, delta: i32) {
        {
            let mut st = self.state.borrow_mut();
            st.anchor = match st.view {
                0 => st.anchor + Duration::days(delta as i64),
                1 => st.anchor + Duration::weeks(delta as i64),
                2 => add_months(month_start(st.anchor), delta),
                _ => st.anchor,
            };
            if st.view == 0 {
                st.selected = st.anchor;
            }
            st.mini_month = month_start(st.anchor);
        }
        self.refresh();
    }

    fn go_today(&self) {
        {
            let mut st = self.state.borrow_mut();
            let today = now_local().date();
            st.anchor = today;
            st.selected = today;
            st.mini_month = month_start(today);
        }
        self.refresh();
    }

    fn set_view(&self, v: i32) {
        self.state.borrow_mut().view = v.clamp(0, 4);
        {
            let mut st = self.state.borrow_mut();
            st.anchor = st.selected;
        }
        self.prefs.borrow_mut().view = v;
        self.save_prefs();
        self.refresh();
    }

    fn select_date(&self, key: &str) {
        let Some(d) = parse_key(key) else { return };
        {
            let mut st = self.state.borrow_mut();
            st.selected = d;
            // Month view keeps its month unless the day belongs to another.
            if st.view != 2 || month_start(d) != month_start(st.anchor) {
                st.anchor = d;
            }
            st.mini_month = month_start(d);
        }
        self.refresh();
    }

    fn quick_add(&self, text: &str, column: Option<String>, default_today: bool) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let lists = self.doc.lock().unwrap().lists();
        let q = quick::parse(text, now_local(), &lists);
        let mut t = Task::blank("", DEFAULT_LIST, column.as_deref().unwrap_or(COL_TODO));
        t.title = q.title.clone();
        t.start = q.start;
        t.end = q.end;
        t.all_day = q.all_day;
        t.priority = q.priority;
        t.repeat = q.repeat;
        t.list = q.list.clone().unwrap_or_else(|| self.default_list(&lists));
        if default_today && t.start.is_none() {
            t.all_day = true;
            t.start = Some(now_local().date().and_time(NaiveTime::MIN));
        }
        let when = q.start.map(|s| self.ctx().when(&t, s));
        self.edit(|d| {
            d.add_task(t);
        });
        self.toast(&match when {
            Some(w) => format!("Added “{}” · {w}", q.title),
            None => format!("Added “{}”", q.title),
        });
    }

    fn default_list(&self, lists: &[patch_core::List]) -> String {
        let hidden = &self.prefs.borrow().hidden_lists;
        lists
            .iter()
            .find(|l| !hidden.contains(&l.id))
            .or(lists.first())
            .map(|l| l.id.clone())
            .unwrap_or_else(|| DEFAULT_LIST.into())
    }

    fn drop_on_date(&self, id: &str, key: &str) {
        let Some(day) = parse_key(key) else { return };
        self.edit(|d| {
            if let Some(t) = d.task(id) {
                let time = t.start.map(|s| s.time()).unwrap_or(NaiveTime::MIN);
                d.reschedule(id, day.and_time(time));
            }
        });
    }

    fn move_block(&self, id: &str, days: i32, minutes: i32) {
        self.edit(|d| {
            if let Some(t) = d.task(id) {
                if let Some(s) = t.start {
                    d.reschedule(id, s + Duration::days(days as i64) + Duration::minutes(minutes as i64));
                }
            }
        });
    }

    fn resize_block(&self, id: &str, minutes: i32) {
        self.edit(|d| {
            if let Some(mut t) = d.task(id) {
                if let (Some(s), Some(e)) = (t.start, t.span_end()) {
                    t.end = Some((e + Duration::minutes(minutes as i64)).max(s + Duration::minutes(15)));
                    d.save_task(&t);
                }
            }
        });
    }

    fn drop_card(&self, id: &str, col: i32, index: i32) {
        let cols = self.doc.lock().unwrap().columns();
        let Some(column) = cols.get(col.max(0) as usize).map(|c| c.id.clone()) else { return };
        // The board hides lists; translate the visible index into a position
        // among all of the column's cards.
        let hidden = self.prefs.borrow().hidden_lists.clone();
        let visible_ids: Vec<String> = self
            .doc
            .lock()
            .unwrap()
            .tasks()
            .into_iter()
            .filter(|t| t.column == column && t.id != id && !hidden.contains(&t.list))
            .map(|t| t.id)
            .collect();
        let all: Vec<String> = self
            .doc
            .lock()
            .unwrap()
            .tasks()
            .into_iter()
            .filter(|t| t.column == column && t.id != id)
            .map(|t| t.id)
            .collect();
        let index = index.max(0) as usize;
        let pos = match visible_ids.get(index) {
            Some(next) => all.iter().position(|x| x == next).unwrap_or(all.len()),
            None => all.len(),
        };
        self.edit(|d| d.move_task(id, &column, pos));
    }

    fn connect_hub(&self, url: &str, token: &str) {
        match SyncConfig::new(url, token) {
            Ok(cfg) => {
                if self.hosted.borrow().is_some() {
                    let _ = self.host.set_hosting(false);
                    *self.hosted.borrow_mut() = None;
                    self.prefs.borrow_mut().hosting = false;
                }
                {
                    let mut p = self.prefs.borrow_mut();
                    p.hub_url = cfg.base.url();
                    p.hub_token = cfg.token.clone();
                }
                self.save_prefs();
                self.sync.set_config(Some(cfg.clone()));
                self.each_store(|s| {
                    s.set_hub_url(cfg.base.url().into());
                    s.set_hub_token(cfg.token.clone().into());
                    s.set_hosting(false);
                });
                self.toast("Connecting over Tailscale…");
            }
            Err(e) => self.toast(&e),
        }
    }

    fn set_hosting(&self, on: bool) {
        match self.host.set_hosting(on) {
            Ok(hub) => {
                self.prefs.borrow_mut().hosting = on && hub.is_some();
                *self.hosted.borrow_mut() = hub.clone();
                self.save_prefs();
                match hub {
                    Some(h) => {
                        self.sync.set_config(SyncConfig::new(&h.local_url, &h.token).ok());
                        self.each_store(|s| {
                            s.set_hosting(true);
                            s.set_host_urls(h.urls.join("\n").into());
                            s.set_host_token(h.token.clone().into());
                        });
                    }
                    None => {
                        self.configure_sync();
                        self.each_store(|s| s.set_hosting(false));
                    }
                }
            }
            Err(e) => {
                self.toast(&e);
                self.each_store(|s| s.set_hosting(!on));
            }
        }
    }

    fn on_sync(&self, ev: SyncEvent) {
        match ev {
            SyncEvent::Status(s) => {
                *self.status.borrow_mut() = s;
                self.each_store(|st| self.apply_status_to(st));
            }
            SyncEvent::RemoteApplied => {
                self.refresh();
                self.save_if_dirty();
            }
        }
    }

    fn apply_status_to(&self, s: &Store) {
        let status = self.status.borrow();
        let (state, label) = match &*status {
            SyncStatus::Disabled => (0, "Local only — host a hub or connect to one below.".to_string()),
            SyncStatus::Connecting | SyncStatus::Syncing => (1, status.label()),
            SyncStatus::Synced(_) => {
                let target = if self.hosted.borrow().is_some() {
                    "this computer".to_string()
                } else {
                    self.sync.config().map(|c| c.base.host.clone()).unwrap_or_default()
                };
                (2, format!("Synced with {target} over Tailscale"))
            }
            SyncStatus::Offline(e) => (3, format!("Offline — {e}")),
            SyncStatus::Unauthorized => (4, "The hub rejected the pairing token".to_string()),
        };
        s.set_sync_state(state);
        s.set_sync_label(label.into());
    }

    fn tick(&self) {
        let today = now_local().date();
        let rolled = {
            let mut st = self.state.borrow_mut();
            if st.today != today {
                // Follow the day forward if the user was looking at "today".
                if st.selected == st.today {
                    st.selected = today;
                    st.anchor = today;
                    st.mini_month = month_start(today);
                }
                st.today = today;
                true
            } else {
                false
            }
        };
        if rolled {
            self.refresh();
        } else {
            self.refresh_clock();
        }
        self.fire_reminders();
    }

    fn fire_reminders(&self) {
        let now = now_local();
        let tasks = self.doc.lock().unwrap().tasks();
        let mut fired = Vec::new();
        {
            let p = self.prefs.borrow();
            for t in tasks.iter().filter(|t| !t.done) {
                let (Some(start), Some(before)) = (t.start, t.reminder) else { continue };
                let at = start - Duration::minutes(before);
                let key = format!("{}@{}", t.id, patch_core::time::fmt_dt(start));
                if at <= now && now < at + Duration::minutes(15) && !p.notified.contains(&key) {
                    fired.push((key, t.clone(), start));
                }
            }
        }
        if fired.is_empty() {
            return;
        }
        let ctx = self.ctx();
        for (key, t, start) in &fired {
            self.host.notify(&t.title, &ctx.when(t, *start));
            self.prefs.borrow_mut().notified.push(key.clone());
        }
        let mut p = self.prefs.borrow_mut();
        let excess = p.notified.len().saturating_sub(300);
        p.notified.drain(..excess);
        drop(p);
        self.save_prefs();
    }

    // ------------------------------------------------------------------ editor

    /// Open the editor for a task (e.g. tapped in the overlay).
    pub fn open_task(&self, id: &str) {
        let Some(task) = self.doc.lock().unwrap().task(id) else { return };
        let month = month_start(task.start.map(|s| s.date()).unwrap_or(self.state.borrow().selected));
        self.state.borrow_mut().editor = Some(EditorState { task, is_new: false, picking_end: false, picker_month: month });
        self.load_editor();
    }

    /// `minutes`: >= 0 timed at that minute, -1 all-day, -2 "+" button.
    fn new_task(&self, key: &str, minutes: i32) {
        let (selected, view) = {
            let st = self.state.borrow();
            (st.selected, st.view)
        };
        let day = parse_key(key).unwrap_or(selected);
        let lists = self.doc.lock().unwrap().lists();
        let mut t = Task::blank(patch_core::id::new_id("t"), &self.default_list(&lists), COL_TODO);
        match minutes {
            m if m >= 0 => {
                t.start = Some(day.and_time(hm(m as u32 / 60, m as u32 % 60)));
                t.end = t.start.map(|s| s + Duration::hours(1));
            }
            -1 => {
                t.all_day = true;
                t.start = Some(day.and_time(NaiveTime::MIN));
            }
            _ if view <= 2 => {
                // Next full hour today, 9:00 on other days.
                let now = now_local();
                let start = if day == now.date() && now.hour() < 23 {
                    day.and_time(hm(now.hour() + 1, 0))
                } else {
                    day.and_time(hm(9, 0))
                };
                t.start = Some(start);
                t.end = Some(start + Duration::hours(1));
            }
            _ => {}
        }
        self.state.borrow_mut().editor = Some(EditorState { task: t, is_new: true, picking_end: true, picker_month: month_start(day) });
        self.load_editor();
    }

    fn load_editor(&self) {
        let (lists, columns) = {
            let d = self.doc.lock().unwrap();
            (d.lists(), d.columns())
        };
        update(&self.models.editor_lists, lists.iter().map(|l| Choice { label: l.name.clone().into(), color: color(&l.color) }).collect());
        update(&self.models.editor_columns, columns.iter().map(|c| Choice { label: c.name.clone().into(), color: slint::Color::default() }).collect());
        let st = self.state.borrow();
        let Some(e) = st.editor.as_ref() else { return };
        let t = &e.task;
        self.with_editor(|ed| {
            ed.set_id(t.id.clone().into());
            ed.set_is_new(e.is_new);
            ed.set_title(t.title.clone().into());
            ed.set_notes(t.notes.clone().into());
            ed.set_list_index(lists.iter().position(|l| l.id == t.list).unwrap_or(0) as i32);
            ed.set_column_index(columns.iter().position(|c| c.id == t.column).unwrap_or(0) as i32);
            ed.set_priority(t.priority as i32);
            ed.set_scheduled(t.start.is_some());
            ed.set_all_day(t.all_day);
            let start = t.start.unwrap_or_else(|| st.selected.and_time(hm(9, 0)));
            let end = t.span_end().unwrap_or(start + Duration::hours(1));
            ed.set_start_key(start.date().to_string().into());
            let end_day = if t.all_day { t.end.unwrap_or(start).date().max(start.date()) } else { end.date() };
            ed.set_end_key(end_day.to_string().into());
            ed.set_start_min(minutes_of(start) as i32);
            ed.set_end_min(if t.all_day { minutes_of(start) as i32 + 60 } else { minutes_of(end) as i32 });
            ed.set_repeat_index(t.repeat.index() as i32);
            ed.set_reminder_index(REMINDER_CHOICES.iter().position(|r| *r == t.reminder).unwrap_or(0) as i32);
        });
        update(&self.models.checklist, checklist_rows(&t.checklist));
        drop(st);
        self.refresh_editor_dates();
        self.each_store(|s| s.set_editor_open(true));
    }

    /// Reads the editor's fields back into the working task.
    fn read_editor(&self) {
        let (lists, columns) = {
            let d = self.doc.lock().unwrap();
            (d.lists(), d.columns())
        };
        let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        let ed = w.global::<Editor>();
        let mut st = self.state.borrow_mut();
        let Some(e) = st.editor.as_mut() else { return };
        let t = &mut e.task;
        t.title = ed.get_title().trim().to_string();
        t.notes = ed.get_notes().to_string();
        if let Some(l) = lists.get(ed.get_list_index().max(0) as usize) {
            t.list = l.id.clone();
        }
        if let Some(c) = columns.get(ed.get_column_index().max(0) as usize) {
            t.column = c.id.clone();
        }
        t.priority = ed.get_priority().clamp(0, 3) as u8;
        t.repeat = Repeat::ALL.get(ed.get_repeat_index().max(0) as usize).copied().unwrap_or_default();
        t.reminder = REMINDER_CHOICES.get(ed.get_reminder_index().max(0) as usize).copied().flatten();
        if !ed.get_scheduled() {
            t.start = None;
            t.end = None;
            t.all_day = false;
            t.repeat = Repeat::None;
            t.reminder = None;
            return;
        }
        let start_day = parse_key(&ed.get_start_key()).unwrap_or_else(|| now_local().date());
        let end_day = parse_key(&ed.get_end_key()).unwrap_or(start_day).max(start_day);
        t.all_day = ed.get_all_day();
        if t.all_day {
            t.start = Some(start_day.and_time(NaiveTime::MIN));
            t.end = (end_day > start_day).then(|| end_day.and_time(NaiveTime::MIN));
        } else {
            let sm = ed.get_start_min().clamp(0, 1439) as u32;
            let em = ed.get_end_min().clamp(0, 1439) as u32;
            let start = start_day.and_time(hm(sm / 60, sm % 60));
            let mut end = end_day.and_time(hm(em / 60, em % 60));
            if end <= start {
                end = start + Duration::hours(1);
            }
            t.start = Some(start);
            t.end = Some(end);
        }
    }

    fn editor_changed(&self) {
        // Turning scheduling on for an undated task starts it on the
        // selected day.
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            let ed = w.global::<Editor>();
            if ed.get_scheduled() && ed.get_start_key().is_empty() {
                let d = self.state.borrow().selected;
                ed.set_start_key(d.to_string().into());
                ed.set_end_key(d.to_string().into());
            }
        }
        self.read_editor();
        self.refresh_editor_dates();
    }

    fn refresh_editor_dates(&self) {
        let monday = self.prefs.borrow().week_monday;
        let today = now_local().date();
        let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        let ed = w.global::<Editor>();
        let st = self.state.borrow();
        let Some(e) = st.editor.as_ref() else { return };
        let start = parse_key(&ed.get_start_key());
        let end = parse_key(&ed.get_end_key());
        ed.set_picker_title(e.picker_month.format("%B %Y").to_string().into());
        update(&self.models.picker, models::picker_cells(e.picker_month, start, end, monday, today));
        // Human summary under the picker.
        let ctx = self.ctx();
        // Absolute dates read better than "Tuesday" in a range summary.
        let day = |d: NaiveDate| match (d - today).num_days() {
            0 => "Today".to_string(),
            1 => "Tomorrow".to_string(),
            _ => d.format("%a %-d %b").to_string(),
        };
        let label = match start {
            None => String::new(),
            Some(s) => {
                let e_day = end.unwrap_or(s).max(s);
                if ed.get_all_day() {
                    if e_day > s {
                        format!("All-day · {} → {} · {} days", day(s), day(e_day), (e_day - s).num_days() + 1)
                    } else {
                        format!("All-day · {}", day(s))
                    }
                } else {
                    let sm = ed.get_start_min().clamp(0, 1439) as u32;
                    let em = ed.get_end_min().clamp(0, 1439) as u32;
                    let sdt = s.and_time(hm(sm / 60, sm % 60));
                    let edt = e_day.and_time(hm(em / 60, em % 60));
                    if e_day > s {
                        format!("{}, {} → {}, {}", day(s), ctx.time(sdt), day(e_day), ctx.time(edt))
                    } else {
                        format!("{} · {} – {}", day(s), ctx.time(sdt), ctx.time(edt))
                    }
                }
            }
        };
        ed.set_range_label(label.into());
    }

    /// First tap picks a day; the second extends it into a range, like the
    /// reference picker.
    fn picker_tap(&self, key: &str) {
        let Some(day) = parse_key(key) else { return };
        let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        let ed = w.global::<Editor>();
        {
            let mut st = self.state.borrow_mut();
            let Some(e) = st.editor.as_mut() else { return };
            let start = parse_key(&ed.get_start_key());
            let end = parse_key(&ed.get_end_key());
            match start {
                Some(s) if e.picking_end && day > s => {
                    ed.set_end_key(day.to_string().into());
                    e.picking_end = false;
                }
                // Re-tapping a single selected day keeps waiting for the end.
                Some(s) if day == s && end.is_none_or(|e| e == s) => e.picking_end = true,
                _ => {
                    ed.set_start_key(day.to_string().into());
                    ed.set_end_key(day.to_string().into());
                    e.picking_end = true;
                }
            }
        }
        self.read_editor();
        self.refresh_editor_dates();
    }

    fn picker_reset(&self) {
        let today = now_local().date();
        if let Some(w) = self.main.borrow().as_ref().and_then(|w| w.upgrade()) {
            let ed = w.global::<Editor>();
            ed.set_start_key(today.to_string().into());
            ed.set_end_key(today.to_string().into());
            let now = now_local();
            let start = ((now.hour() + 1).min(23) * 60) as i32;
            ed.set_start_min(start);
            ed.set_end_min((start + 60).min(1439));
        }
        if let Some(e) = self.state.borrow_mut().editor.as_mut() {
            e.picking_end = true;
            e.picker_month = month_start(today);
        }
        self.read_editor();
        self.refresh_editor_dates();
    }

    fn edit_checklist(&self, f: impl FnOnce(&mut Vec<CheckItem>)) {
        self.read_editor();
        let rows = {
            let mut st = self.state.borrow_mut();
            let Some(e) = st.editor.as_mut() else { return };
            f(&mut e.task.checklist);
            e.task.checklist.retain(|c| !c.text.is_empty());
            checklist_rows(&e.task.checklist)
        };
        update(&self.models.checklist, rows);
    }

    fn close_editor(&self, save: bool) {
        if save {
            self.read_editor();
        }
        let editor = self.state.borrow_mut().editor.take();
        self.each_store(|s| s.set_editor_open(false));
        if let (true, Some(e)) = (save, editor) {
            if !e.task.title.is_empty() {
                let is_new = e.is_new;
                let title = e.task.title.clone();
                self.edit(|d| d.save_task(&e.task));
                if is_new {
                    self.toast(&format!("Added “{title}”"));
                }
                return;
            }
        }
        self.refresh();
    }

    // ------------------------------------------------------------------ refresh

    fn ctx(&self) -> Ctx {
        let d = self.doc.lock().unwrap();
        let p = self.prefs.borrow();
        Ctx {
            tasks: d.tasks(),
            lists: d.lists().into_iter().map(|l| (l.id.clone(), l)).collect::<HashMap<_, _>>(),
            hidden: p.hidden_lists.iter().cloned().collect::<HashSet<_>>(),
            now: now_local(),
            h24: p.time_24h,
            monday: p.week_monday,
        }
    }

    fn refresh_clock(&self) {
        let now = now_local();
        let p = self.prefs.borrow();
        let clock = patch_core::time::fmt_time(now, p.time_24h);
        let clock = if p.time_24h { clock } else { clock.to_uppercase() };
        drop(p);
        let today_label = now.format("%A, %-d %B").to_string();
        let days: Vec<NaiveDate> = self.models.days.iter().filter_map(|d| parse_key(&d.key)).collect();
        let now_day = days.iter().position(|d| *d == now.date()).map(|i| i as i32).unwrap_or(-1);
        self.each_store(|s| {
            s.set_clock(clock.clone().into());
            s.set_today_label(today_label.clone().into());
            s.set_now_minutes(minutes_of(now) as f32);
            s.set_now_day(now_day);
        });
    }

    /// Recomputes every view model from the document and UI state.
    pub fn refresh(&self) {
        let ctx = self.ctx();
        let (columns, all_lists) = {
            let d = self.doc.lock().unwrap();
            (d.columns(), d.lists())
        };
        let st = self.state.borrow();
        let (view, anchor, selected, mini) = (st.view, st.anchor, st.selected, st.mini_month);
        let search = st.search.clone();
        drop(st);

        let weekdays: Vec<NaiveDate> = models::week_days(anchor, ctx.monday);
        update(&self.models.weekdays, weekdays.iter().map(|d| d.format("%a").to_string().to_uppercase().into()).collect());
        update(&self.models.weekdays_short, weekdays.iter().map(|d| d.format("%a").to_string().into()).collect());
        update(&self.models.hours, (0..24).map(|h| fmt_hour_label(h, ctx.h24).into()).collect());

        update(&self.models.mini, models::mini_cells(&ctx, mini, selected));
        let max_items = if self.host.is_mobile() { 4 } else { 8 };
        update(&self.models.month, models::month_cells(&ctx, anchor, selected, max_items));
        update(&self.models.agenda, models::agenda(&ctx, selected));

        let grid_days = if view == 0 { vec![anchor] } else { weekdays.clone() };
        let grid = models::time_grid(&ctx, &grid_days);
        let lanes = grid.lanes;
        update(&self.models.days, grid.days);
        update(&self.models.blocks, grid.blocks);
        update(&self.models.bars, grid.bars);

        let board = models::board(&ctx, &columns);
        let tallest = board.iter().map(|c| c.count).max().unwrap_or(0);
        let open_cards: i32 = board.iter().filter(|c| !c.done).map(|c| c.count).sum();
        update(&self.models.board, board);

        let theme_colors = (color("#FF453A"), color("#0A84FF"), slint::Color::from_rgb_u8(0x9a, 0x9a, 0xa1));
        let sections = models::sections(&ctx, theme_colors);
        let open_tasks: usize = sections.iter().filter(|s| s.title != "Completed").map(|s| s.items.row_count()).sum();
        update(&self.models.sections, sections);
        update(&self.models.up_next, models::up_next(&ctx, 6));
        let (today_items, today_open) = models::today_items(&ctx);
        update(&self.models.today, today_items);
        update(&self.models.results, models::search(&ctx, &search));

        let counts: HashMap<&str, i32> = ctx.tasks.iter().filter(|t| !t.done).fold(HashMap::new(), |mut m, t| {
            *m.entry(t.list.as_str()).or_insert(0) += 1;
            m
        });
        update(
            &self.models.lists,
            all_lists
                .iter()
                .map(|l| ListInfo {
                    id: l.id.clone().into(),
                    name: l.name.clone().into(),
                    color: color(&l.color),
                    visible: !ctx.hidden.contains(&l.id),
                    count: counts.get(l.id.as_str()).copied().unwrap_or(0),
                })
                .collect(),
        );

        let (title_main, title_sub, caption) = match view {
            0 => (anchor.format("%-d %B").to_string(), anchor.format("%Y").to_string(), anchor.format("%A").to_string()),
            1 => {
                let (a, b) = (weekdays[0], weekdays[6]);
                let main = if a.month() == b.month() {
                    a.format("%B").to_string()
                } else {
                    format!("{} – {}", a.format("%b"), b.format("%b"))
                };
                (main, b.format("%Y").to_string(), format!("Week {}", a.iso_week().week()))
            }
            2 => (anchor.format("%B").to_string(), anchor.format("%Y").to_string(), String::new()),
            3 => ("Board".into(), String::new(), format!("{open_cards} open")),
            _ => ("Tasks".into(), String::new(), format!("{open_tasks} open")),
        };
        let agenda_title = format!("{} · {}", ctx.day_name(selected).to_uppercase(), selected.format("%-d %B").to_string().to_uppercase());
        self.each_store(|s| {
            s.set_view(view);
            s.set_title_main(title_main.clone().into());
            s.set_title_sub(title_sub.clone().into());
            s.set_caption(caption.clone().into());
            s.set_selected(selected.to_string().into());
            s.set_mini_title(mini.format("%B %Y").to_string().into());
            s.set_grid_lanes(lanes);
            s.set_board_tallest(tallest);
            s.set_today_open(today_open as i32);
            s.set_day_agenda_title(agenda_title.clone().into());
        });
        self.refresh_clock();
    }
}

fn checklist_rows(items: &[CheckItem]) -> Vec<CheckRow> {
    items
        .iter()
        .map(|c| CheckRow { id: c.id.clone().into(), text: c.text.clone().into(), done: c.done })
        .collect()
}
