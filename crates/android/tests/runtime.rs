//! Drives the Android runtime exactly as the Java side does — surfaces,
//! redraw requests, touches, IME edits — on the desktop, with in-memory
//! surfaces, and writes what it rendered to `screenshots/android-*.png`.
//!
//! One test: Slint allows a single platform per process.

use chrono::Duration as Days;
use patch_android::buffer::{BackBuffer, Rect};
use patch_android::ime::{ImeMode, ImeOp, ImeSnapshot};
use patch_android::input::TouchAction;
use patch_android::runtime::{Bridge, Kind, Msg, Runtime, Surface};
use patch_core::doc::{COL_DOING, COL_TODO, DEFAULT_LIST, LIST_WORK};
use patch_core::time::{hm, today};
use patch_core::{Doc, Task};
use patch_ui::{Actions, Store};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SCALE: f32 = 2.625;

#[derive(Default)]
struct Recorder {
    ime: RefCell<Vec<(Kind, ImeMode, ImeSnapshot, i32)>>,
    frames: RefCell<Vec<Kind>>,
    expand: RefCell<Vec<bool>>,
    back: RefCell<Vec<bool>>,
    looks: RefCell<Vec<i32>>,
    clipboard: RefCell<Option<String>>,
    overlay: Cell<bool>,
    reminders: RefCell<Vec<String>>,
    snapshots: RefCell<Vec<String>>,
    updates: Cell<u32>,
    battery_settings: Cell<u32>,
}

impl Bridge for Recorder {
    fn set_overlay(&self, on: bool) -> bool {
        self.overlay.set(on);
        true
    }
    fn overlay_allowed(&self) -> bool {
        true
    }
    fn set_overlay_boot(&self, _on: bool) {}
    fn overlay_expand(&self, expanded: bool) {
        self.expand.borrow_mut().push(expanded);
    }
    fn open_app(&self, _task_id: Option<&str>) {}
    fn notify(&self, _title: &str, _body: &str) {}
    fn schedule_reminders(&self, json: &str) {
        self.reminders.borrow_mut().push(json.to_string());
    }
    fn set_clipboard(&self, text: &str) {
        *self.clipboard.borrow_mut() = Some(text.into());
    }
    fn clipboard(&self) -> Option<String> {
        self.clipboard.borrow().clone()
    }
    fn ime(&self, kind: Kind, mode: ImeMode, state: &ImeSnapshot, seq: i32) {
        self.ime.borrow_mut().push((kind, mode, state.clone(), seq));
    }
    fn frame_done(&self, kind: Kind) {
        self.frames.borrow_mut().push(kind);
    }
    fn back_result(&self, consumed: bool) {
        self.back.borrow_mut().push(consumed);
    }
    fn set_look(&self, look: i32) {
        self.looks.borrow_mut().push(look);
    }
    fn publish_snapshot(&self, json: &str) {
        self.snapshots.borrow_mut().push(json.to_string());
    }
    fn scan_qr(&self) {}
    fn check_update(&self) {
        self.updates.set(self.updates.get() + 1);
    }
    fn battery_unrestricted(&self) -> bool {
        false
    }
    fn request_battery_unrestricted(&self) {}
    fn battery_usage(&self) -> Option<String> {
        Some("Device battery: 76% · draining 120 mA".into())
    }
    fn open_battery_settings(&self) {
        self.battery_settings.set(self.battery_settings.get() + 1);
    }
}

/// Stands in for an ANativeWindow: a tightly packed RGBA buffer.
#[derive(Clone)]
struct MemSurface {
    width: u32,
    height: u32,
    pixels: Arc<Mutex<Vec<u8>>>,
    presents: Arc<Mutex<u32>>,
}

impl MemSurface {
    fn new(width: u32, height: u32) -> MemSurface {
        MemSurface {
            width,
            height,
            pixels: Arc::new(Mutex::new(vec![0; (width * height * 4) as usize])),
            presents: Arc::default(),
        }
    }

    fn presents(&self) -> u32 {
        *self.presents.lock().unwrap()
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let p = self.pixels.lock().unwrap();
        let i = ((y * self.width + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    fn save(&self, name: &str) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join(name)).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        let mut writer = enc.write_header().unwrap();
        let bytes: Vec<u8> = self
            .pixels
            .lock()
            .unwrap()
            .chunks(4)
            .flat_map(|p| {
                let a = p[3] as u32;
                let un = |c: u8| {
                    (c as u32 * 255 + a / 2)
                        .checked_div(a)
                        .map_or(0, |v| v.min(255) as u8)
                };
                [un(p[0]), un(p[1]), un(p[2]), p[3]]
            })
            .collect();
        writer.write_image_data(&bytes).unwrap();
    }
}

impl Surface for MemSurface {
    fn present(&mut self, frame: &BackBuffer, rect: Rect) -> Result<(), String> {
        let mut p = self.pixels.lock().unwrap();
        // SAFETY: the buffer is width*height RGBA pixels with stride = width.
        unsafe { frame.copy_to(rect, p.as_mut_ptr(), self.width, self.height, self.width) };
        *self.presents.lock().unwrap() += 1;
        Ok(())
    }
}

fn seed(dir: &Path) {
    let mut d = Doc::with_node("android-test");
    let t0 = today();
    let mut add = |title: &str, list: &str, col: &str, f: &dyn Fn(&mut Task)| {
        let mut t = Task::blank("", list, col);
        t.title = title.into();
        f(&mut t);
        d.add_task(t);
    };
    add("Design review", LIST_WORK, COL_DOING, &|t| {
        t.start = Some(t0.and_time(hm(10, 0)));
        t.end = Some(t0.and_time(hm(11, 30)));
        t.priority = 2;
    });
    add("Gym", DEFAULT_LIST, COL_TODO, &|t| {
        t.start = Some(t0.and_time(hm(18, 30)));
        t.end = Some(t0.and_time(hm(19, 30)));
    });
    add("Pay rent", DEFAULT_LIST, COL_TODO, &|t| {
        t.start = Some(t0.and_time(hm(0, 0)));
        t.all_day = true;
        t.priority = 3;
    });
    add("Lunch with Sam", DEFAULT_LIST, COL_TODO, &|t| {
        t.start = Some((t0 + Days::days(1)).and_time(hm(13, 0)));
        t.end = Some((t0 + Days::days(1)).and_time(hm(14, 0)));
        t.reminder = Some(15);
    });
    add("Ship Android build", LIST_WORK, COL_DOING, &|t| {
        t.start = Some((t0 + Days::days(2)).and_time(hm(15, 0)));
        t.end = Some((t0 + Days::days(2)).and_time(hm(17, 0)));
        t.priority = 3;
    });
    add("Plan trip to Kyoto", DEFAULT_LIST, COL_TODO, &|_| {});
    d.save(&dir.join("doc.json")).unwrap();
}

struct Harness {
    rt: Runtime,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    rec: Rc<Recorder>,
    dir: PathBuf,
}

impl Harness {
    /// Runs the loop like the UI thread does until animations settle.
    fn settle(&mut self) {
        for _ in 0..14 {
            self.rt.step(&self.rx, Some(Duration::ZERO));
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// Samples the close animation before Java would shrink the surface.
    fn animate_partway(&mut self) {
        for _ in 0..5 {
            self.rt.step(&self.rx, Some(Duration::ZERO));
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn send(&self, msg: Msg) {
        self.tx.send(msg).unwrap();
    }

    fn tap(&mut self, kind: Kind, x: f32, y: f32) {
        let (x, y) = (x * SCALE, y * SCALE);
        self.send(Msg::Touch {
            kind,
            action: TouchAction::Down,
            x,
            y,
        });
        self.send(Msg::Touch {
            kind,
            action: TouchAction::Up,
            x,
            y,
        });
        self.settle();
    }

    fn surface(&mut self, kind: Kind, logical: (f32, f32)) -> MemSurface {
        let s = MemSurface::new(
            (logical.0 * SCALE).round() as u32,
            (logical.1 * SCALE).round() as u32,
        );
        self.send(Msg::Surface {
            kind,
            surface: Box::new(s.clone()),
            width: s.width,
            height: s.height,
        });
        self.send(Msg::RedrawNeeded(kind));
        s
    }

    fn last_ime(&self) -> Option<(Kind, ImeMode, ImeSnapshot, i32)> {
        self.rec.ime.borrow().last().cloned()
    }

    fn task_titles(&self) -> Vec<String> {
        self.rt.app().save_now();
        Doc::load_or_new(&self.dir.join("doc.json"))
            .tasks()
            .into_iter()
            .map(|t| t.title)
            .collect()
    }
}

#[test]
fn renders_and_drives_both_windows() {
    let dir = std::env::temp_dir().join(format!("patch-android-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    seed(&dir);

    let (tx, rx) = channel();
    let rec = Rc::new(Recorder::default());
    let rt = Runtime::new(dir.clone(), SCALE, rec.clone(), tx.clone()).unwrap();
    let mut h = Harness {
        rt,
        rx,
        tx,
        rec,
        dir,
    };

    // ---- The activity: a 412×915dp phone at 2.625x with system bars.
    h.send(Msg::Insets {
        top: 24.0 * SCALE,
        bottom: 16.0 * SCALE,
    });
    let main = h.surface(Kind::Main, (412.0, 915.0));
    h.settle();
    assert_eq!(
        h.rec.frames.borrow().as_slice(),
        [Kind::Main],
        "redraw-needed is acknowledged once"
    );
    assert!(main.presents() >= 1);
    // Reminders go to AlarmManager so they survive the process being killed.
    let scheduled = h.rec.reminders.borrow().last().cloned().expect("reminders handed to Java");
    let list: serde_json::Value = serde_json::from_str(&scheduled).unwrap();
    let lunch = list.as_array().unwrap().iter().find(|r| r["title"] == "Lunch with Sam").expect("lunch reminder");
    let due = (today() + Days::days(1)).and_time(hm(12, 45));
    let due_ms = chrono::TimeZone::from_local_datetime(&chrono::Local, &due).earliest().unwrap().timestamp_millis();
    assert_eq!(lunch["at"].as_i64(), Some(due_ms), "fires 15 minutes before the start");
    assert_eq!(lunch["body"], "Tomorrow, 13:00");
    // The home-screen widget gets today's agenda as JSON.
    let snapshot = h.rec.snapshots.borrow().last().cloned().expect("widget snapshot handed to Java");
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    let today_titles: Vec<&str> = snapshot["today"]
        .as_array()
        .expect("today list")
        .iter()
        .filter_map(|t| t["title"].as_str())
        .collect();
    assert!(today_titles.contains(&"Gym"), "today's tasks in the snapshot: {snapshot}");
    assert!(snapshot["upcoming"].is_array() && snapshot["date"].is_string() && snapshot["mood"].is_string());
    assert_eq!(
        h.rec.looks.borrow().first(),
        Some(&0),
        "matte look reported to Java"
    );
    let mw = h.rt.main_window().expect("main window created");
    assert!(mw.global::<Store>().get_mobile(), "phone layout");
    assert!(mw.global::<Store>().get_can_update(), "Android supports updates");
    mw.global::<Actions>().invoke_check_update();
    assert_eq!(h.rec.updates.get(), 1, "update action reaches the host");
    mw.global::<Actions>().invoke_refresh_usage();
    assert_eq!(mw.global::<Store>().get_battery_usage(), "Device battery: 76% · draining 120 mA");
    mw.global::<Actions>().invoke_battery_settings();
    assert_eq!(h.rec.battery_settings.get(), 1, "battery settings action reaches the bridge");
    assert_eq!(
        main.pixel(main.width / 2, 2)[3],
        255,
        "opaque backdrop under the status bar"
    );
    main.save("android-main-month.png");
    for (view, name) in [(0, "day"), (3, "board")] {
        mw.global::<Actions>().invoke_set_view(view);
        h.settle();
        main.save(&format!("android-main-{name}.png"));
    }
    mw.global::<Actions>().invoke_set_view(2);

    // Back with nothing open: not consumed, Java moves the task to the back.
    h.send(Msg::Back);
    h.settle();
    assert_eq!(h.rec.back.borrow().as_slice(), [false]);

    // The widget's "+" opens the editor on a new task; Back closes it.
    h.send(Msg::NewTask);
    h.settle();
    h.send(Msg::Back);
    h.settle();
    assert_eq!(h.rec.back.borrow().as_slice(), [false, true], "editor was open");

    // Surface lost and recreated (pause/resume): the kept frame is presented
    // again in full without waiting for a change.
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    h.send(Msg::SurfaceLost {
        kind: Kind::Main,
        done,
    });
    h.settle();
    wait.try_recv().expect("surface release acknowledged");
    let main2 = h.surface(Kind::Main, (412.0, 915.0));
    h.settle();
    assert!(main2.presents() >= 1);
    assert_eq!(
        main2.pixel(main2.width / 2, main2.height / 2),
        main.pixel(main.width / 2, main.height / 2)
    );

    // ---- The overlay panel (340×500dp).
    let panel = h.surface(Kind::Overlay, (340.0, 500.0));
    h.settle();
    assert!(h.rt.overlay_window().is_some());
    assert!(
        h.rec.expand.borrow().is_empty(),
        "Java's size already matches the app's state"
    );
    assert!(
        (95..=110).contains(&panel.pixel(0, 0)[3]),
        "the single backdrop dims behind the pet: {:?}",
        panel.pixel(0, 0)
    );
    assert!(
        panel.pixel(panel.width / 2, panel.height / 2)[3] > 200,
        "panel body is drawn"
    );
    panel.save("android-overlay-panel.png");

    // Typing into the quick-add field through the IME path.
    h.tap(Kind::Overlay, 150.0, 474.0);
    let (kind, mode, ..) = h
        .last_ime()
        .expect("tapping the field asks for the keyboard");
    assert_eq!((kind, mode), (Kind::Overlay, ImeMode::Show));
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::Compose("Buy".into(), 1),
        seq: 1,
    });
    h.settle();
    let (_, _, snap, seq) = h.last_ime().unwrap();
    assert_eq!(
        (
            snap.text.as_str(),
            snap.comp_start,
            snap.comp_end,
            snap.sel_end,
            seq
        ),
        ("Buy", 0, 3, 3, 1)
    );
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::Commit("Buy ".into(), 1),
        seq: 2,
    });
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::Compose("milk".into(), 1),
        seq: 3,
    });
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::DeleteSurrounding {
            before: 1,
            after: 0,
        },
        seq: 4,
    });
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::Commit(" milk".into(), 1),
        seq: 5,
    });
    h.settle();
    let (_, _, snap, seq) = h.last_ime().unwrap();
    assert_eq!(
        (snap.text.as_str(), snap.comp_start, seq),
        ("Buy milk", -1, 5)
    );
    h.send(Msg::Ime {
        kind: Kind::Overlay,
        op: ImeOp::EditorAction,
        seq: 6,
    });
    h.settle();
    assert!(
        h.task_titles().iter().any(|t| t == "Buy milk"),
        "Done on the keyboard adds the task"
    );
    let (_, _, snap, _) = h.last_ime().unwrap();
    assert_eq!(snap.text, "", "the cleared field is echoed back to Java");
    panel.save("android-overlay-panel-added.png");

    // ---- Collapse keeps painting the exit animation on the full-size surface.
    h.rt.overlay_window()
        .unwrap()
        .global::<Actions>()
        .invoke_overlay_expand(false);
    let before = panel.presents();
    h.animate_partway();
    assert_eq!(h.rec.expand.borrow().as_slice(), [false]);
    assert!(
        panel.presents() > before,
        "close animation keeps producing frames"
    );
    h.settle();
    assert_eq!(
        panel.pixel(panel.width / 2, panel.height / 2)[3],
        0,
        "panel and dim finish fading before the window shrinks"
    );
    let bubble = h.surface(Kind::Overlay, (60.0, 60.0));
    h.settle();
    assert!(bubble.presents() >= 1);
    assert_eq!(bubble.pixel(0, 0)[3], 0, "transparent around the bubble");
    assert!(
        bubble.pixel(bubble.width / 2, bubble.height / 2)[3] > 200,
        "bubble is drawn"
    );
    bubble.save("android-overlay-bubble.png");

    // Native play changes only the overlay sprite, never opens a panel, and recovers.
    let pixels = |image: slint::Image| image.to_rgba8().unwrap().as_slice().to_vec();
    let play_overlay = h.rt.overlay_window().unwrap();
    let overlay_store = play_overlay.global::<Store>();
    let main_store = mw.global::<Store>();
    for code in [1, -1, 2, -2, 3, -3, 4, 5, 0, i32::MIN] {
        h.send(Msg::PetPlay(code));
        h.rt.step(&h.rx, Some(Duration::ZERO));
        let play = patch_ui::pet::Play::from_code(code);
        let actual = pixels(overlay_store.get_pet_image());
        let normal = pixels(main_store.get_pet_image());
        if play == patch_ui::pet::Play::Idle {
            assert_eq!(actual, normal, "idle and invalid codes restore the mood");
        } else {
            assert_ne!(actual, normal, "play pose reaches only the overlay: {code}");
        }
        assert_eq!(h.rec.expand.borrow().as_slice(), [false], "playing never opens");
    }
    h.send(Msg::PetPlay(3));
    h.settle();
    let flying = pixels(overlay_store.get_pet_image());
    assert_ne!(flying, pixels(main_store.get_pet_image()), "bump returns to flight");
    h.send(Msg::PetPlay(4));
    h.settle();
    assert_eq!(pixels(overlay_store.get_pet_image()), pixels(main_store.get_pet_image()), "landing ends");
    h.send(Msg::PetPlay(5));
    for _ in 0..3 { h.settle(); }
    assert_eq!(pixels(overlay_store.get_pet_image()), pixels(main_store.get_pet_image()), "dizziness ends");
    h.send(Msg::PetPlay(3));
    h.send(Msg::PetPlay(-1));
    h.settle();
    assert_ne!(pixels(overlay_store.get_pet_image()), flying, "catch cancels bump recovery");
    h.send(Msg::PetPlay(0));
    h.rt.step(&h.rx, Some(Duration::ZERO));

    // A cancelled press must not open, and a double tap must open only once.
    for action in [TouchAction::Down, TouchAction::Cancel] {
        h.send(Msg::Touch {
            kind: Kind::Overlay,
            action,
            x: 30.0 * SCALE,
            y: 30.0 * SCALE,
        });
    }
    h.settle();
    assert_eq!(h.rec.expand.borrow().as_slice(), [false]);
    for _ in 0..2 {
        for action in [TouchAction::Down, TouchAction::Up] {
            h.send(Msg::Touch {
                kind: Kind::Overlay,
                action,
                x: 30.0 * SCALE,
                y: 30.0 * SCALE,
            });
        }
    }
    h.settle();
    assert_eq!(h.rec.expand.borrow().as_slice(), [false, true]);
    assert_eq!(
        bubble.pixel(0, 0)[3],
        0,
        "no full-screen content painted into a bubble surface"
    );

    // A dropped pet near the bottom opens its panel above, including with the IME up.
    h.send(Msg::OverlayPosition { x: 160.0, y: 420.0 });
    let floating = h.surface(Kind::Overlay, (340.0, 500.0));
    h.settle();
    let overlay = h.rt.overlay_window().unwrap();
    assert_eq!((overlay.get_pet_x(), overlay.get_pet_y()), (160.0, 420.0));
    assert!(
        floating.pixel((190.0 * SCALE) as u32, (450.0 * SCALE) as u32)[3] > 200,
        "pet stays at the dropped position"
    );
    assert!(
        floating.pixel((170.0 * SCALE) as u32, (100.0 * SCALE) as u32)[3] > 200,
        "panel opens above a pet near the bottom"
    );
    let keyboard = h.surface(Kind::Overlay, (340.0, 300.0));
    h.settle();
    assert!(
        keyboard.pixel((190.0 * SCALE) as u32, (270.0 * SCALE) as u32)[3] > 200,
        "pet is clamped above the keyboard"
    );
    let restored = h.surface(Kind::Overlay, (340.0, 500.0));
    h.settle();
    assert!(
        restored.pixel((190.0 * SCALE) as u32, (450.0 * SCALE) as u32)[3] > 200,
        "keyboard dismissal restores the dropped position"
    );

    // Hiding from the notification turns the preference off.
    h.rec.overlay.set(true);
    h.send(Msg::OverlayClosed);
    h.settle();
    assert!(!h.rec.overlay.get());
    assert!(!h.rt.app().prefs().overlay);
    h.send(Msg::PetPlay(5));
    h.send(Msg::ViewGone(Kind::Overlay));
    h.settle();
    assert!(h.rt.overlay_window().is_none());
    let _recreated = h.surface(Kind::Overlay, (340.0, 500.0));
    h.settle();
    assert_eq!(
        pixels(h.rt.overlay_window().unwrap().global::<Store>().get_pet_image()),
        pixels(main_store.get_pet_image()),
        "destroying the overlay clears play and its recovery timer"
    );

    let _ = std::fs::remove_dir_all(&h.dir);
}
