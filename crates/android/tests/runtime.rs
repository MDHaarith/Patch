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
    fn battery_unrestricted(&self) -> bool {
        false
    }
    fn request_battery_unrestricted(&self) {}
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

    /// Runs the loop for ~150ms: well inside the runtime's 400ms resize hold,
    /// which a full `settle` (~420ms) would outlast.
    fn settle_within_hold(&mut self) {
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
        panel.pixel(0, 0)[3] < 32,
        "rounded corner is see-through: {:?}",
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

    // ---- Collapse to the bubble: frames are held until the surface shrinks.
    h.rt.overlay_window()
        .unwrap()
        .global::<Actions>()
        .invoke_overlay_expand(false);
    let before = panel.presents();
    h.settle_within_hold();
    assert_eq!(h.rec.expand.borrow().as_slice(), [false]);
    assert_eq!(
        panel.presents(),
        before,
        "nothing drawn into the panel-sized surface"
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

    // A tap on the bubble expands it again.
    h.tap(Kind::Overlay, 30.0, 30.0);
    assert_eq!(h.rec.expand.borrow().as_slice(), [false, true]);

    // Hiding from the notification turns the preference off.
    h.rec.overlay.set(true);
    h.send(Msg::OverlayClosed);
    h.settle();
    assert!(!h.rec.overlay.get());
    assert!(!h.rt.app().prefs().overlay);
    h.send(Msg::ViewGone(Kind::Overlay));
    h.settle();
    assert!(h.rt.overlay_window().is_none());

    let _ = std::fs::remove_dir_all(&h.dir);
}
