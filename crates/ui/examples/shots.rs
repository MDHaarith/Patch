//! Renders every screen of the shared UI to PNGs with Slint's software
//! renderer (the renderer the Android app uses), for visual review and CI.
//!
//!     cargo run -p patch-ui --example shots -- [out-dir]

use chrono::{Duration, NaiveDate};
use patch_core::doc::{COL_DOING, COL_DONE, COL_TODO, DEFAULT_LIST, LIST_WORK};
use patch_core::model::{CheckItem, Repeat, Task};
use patch_core::time::{hm, today};
use patch_core::Doc;
use patch_ui::{Actions, App, Host, MainWindow, OverlayWindow, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct ShotPlatform {
    windows: RefCell<Vec<Rc<MinimalSoftwareWindow>>>,
}

impl Platform for ShotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(w.clone());
        Ok(w)
    }
}

struct DemoHost {
    mobile: bool,
}

impl Host for DemoHost {
    fn is_mobile(&self) -> bool {
        self.mobile
    }
    fn can_host_hub(&self) -> bool {
        !self.mobile
    }
    fn custom_chrome(&self) -> bool {
        !self.mobile
    }
}

fn seed(dir: &Path) {
    let mut d = Doc::with_node("demo");
    let t0 = today();
    let day = |offset: i64| t0 + Duration::days(offset);
    let mut add = |title: &str, list: &str, col: &str, f: &dyn Fn(&mut Task)| {
        let mut t = Task::blank("", list, col);
        t.title = title.into();
        f(&mut t);
        d.add_task(t)
    };
    let at = |date: NaiveDate, h: u32, m: u32| Some(date.and_time(hm(h, m)));
    let work = d_list_ids();
    add("Design review", work.1, COL_DOING, &|t| {
        t.start = at(day(0), 10, 0);
        t.end = at(day(0), 11, 30);
        t.priority = 2;
        t.checklist = vec![
            CheckItem { id: "a".into(), text: "Walk through flows".into(), done: true },
            CheckItem { id: "b".into(), text: "Collect feedback".into(), done: false },
        ];
    });
    add("Standup", work.1, COL_TODO, &|t| {
        t.start = at(day(-3), 9, 30);
        t.end = at(day(-3), 9, 45);
        t.repeat = Repeat::Weekdays;
    });
    add("Lunch with Sam", work.0, COL_TODO, &|t| {
        t.start = at(day(1), 13, 0);
        t.end = at(day(1), 14, 0);
    });
    add("Team offsite", work.1, COL_TODO, &|t| {
        t.start = at(day(5), 0, 0);
        t.end = at(day(7), 0, 0);
        t.all_day = true;
    });
    add("Gym", work.0, COL_TODO, &|t| {
        t.start = at(day(0), 18, 30);
        t.end = at(day(0), 19, 30);
        t.repeat = Repeat::Daily;
    });
    add("Ship Android build", work.1, COL_DOING, &|t| {
        t.start = at(day(2), 15, 0);
        t.end = at(day(2), 17, 0);
        t.priority = 3;
        t.notes = "Overlay + sync over Tailscale".into();
    });
    add("Pay rent", work.0, COL_TODO, &|t| {
        t.start = at(day(-1), 0, 0);
        t.all_day = true;
        t.priority = 3;
    });
    add("Dentist", work.0, COL_TODO, &|t| {
        t.start = at(day(3), 16, 0);
        t.end = at(day(3), 16, 45);
        t.reminder = Some(60);
    });
    add("Sync with design", work.1, COL_TODO, &|t| {
        t.start = at(day(0), 10, 30);
        t.end = at(day(0), 11, 0);
    });
    add("Plan trip to Kyoto", work.0, COL_TODO, &|_| {});
    add("Read “Shape Up”", work.0, COL_TODO, &|t| t.priority = 1);
    add("Refactor sync client", work.1, COL_TODO, &|_| {});
    add("Write release notes", work.1, COL_DONE, &|_| {});
    add("Book flights", work.0, COL_DONE, &|_| {});
    add("Quarterly planning", work.1, COL_TODO, &|t| {
        t.start = at(day(9), 0, 0);
        t.end = at(day(11), 0, 0);
        t.all_day = true;
    });
    let ids: Vec<String> = d.tasks().iter().filter(|t| t.column == COL_DONE).map(|t| t.id.clone()).collect();
    for id in ids {
        d.set_done(&id, true);
    }
    d.save(&dir.join("doc.json")).unwrap();
}

fn d_list_ids() -> (&'static str, &'static str) {
    (DEFAULT_LIST, LIST_WORK)
}

fn render(platform: &ShotPlatform, index: usize, size: (u32, u32), scale: f32, out: &Path) {
    let w = platform.windows.borrow()[index].clone();
    w.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    w.set_size(PhysicalSize::new(size.0, size.1));
    // Let property animations (theme cross-fades, toggles) settle.
    for _ in 0..12 {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    slint::platform::update_timers_and_animations();
    w.request_redraw();
    let mut buf = vec![PremultipliedRgbaColor::default(); (size.0 * size.1) as usize];
    w.draw_if_needed(|r| {
        r.render(&mut buf, size.0 as usize);
    });
    let file = std::fs::File::create(out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    enc.set_color(png::ColorType::Rgba);
    let mut writer = enc.write_header().unwrap();
    let bytes: Vec<u8> = buf
        .iter()
        .flat_map(|p| {
            // Un-premultiply for PNG.
            let a = p.alpha as u32;
            let un = |c: u8| (c as u32 * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
            [un(p.red), un(p.green), un(p.blue), p.alpha]
        })
        .collect();
    writer.write_image_data(&bytes).unwrap();
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screenshots".into()));
    std::fs::create_dir_all(&out).unwrap();
    let platform = Rc::new(ShotPlatform { windows: RefCell::new(Vec::new()) });
    struct Shared(Rc<ShotPlatform>);
    impl Platform for Shared {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            self.0.create_window_adapter()
        }
    }
    slint::platform::set_platform(Box::new(Shared(platform.clone()))).unwrap();
    let only = std::env::var("SHOTS").unwrap_or_default();
    let want = |name: &str| only.is_empty() || only.split(',').any(|o| name.starts_with(o));
    let looks = ["matte", "glass", "light", "midnight", "aurora", "sunset", "forest", "graphite", "paper", "rose"];

    // Desktop
    let dir = std::env::temp_dir().join(format!("patch-shots-desktop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    seed(&dir);
    let app = App::new(dir.clone(), Rc::new(DemoHost { mobile: false }));
    let main = MainWindow::new().unwrap();
    app.attach_main(&main);
    let idx = platform.windows.borrow().len() - 1;
    main.show().unwrap();
    let a = main.global::<Actions>();
    for (li, look) in looks.iter().enumerate() {
        a.invoke_set_look(li as i32);
        for (vi, view) in ["day", "week", "month", "board", "list"].iter().enumerate() {
            let name = format!("desktop-{look}-{view}");
            if want(&name) {
                a.invoke_set_view(vi as i32);
                render(&platform, idx, (1440, 900), 1.0, &out.join(format!("{name}.png")));
            }
        }
        let name = format!("desktop-{look}-editor");
        if want(&name) {
            a.invoke_set_view(2);
            let id = app_task(&dir, "Team offsite");
            a.invoke_open_task(id.into());
            render(&platform, idx, (1440, 900), 1.0, &out.join(format!("{name}.png")));
            a.invoke_editor_close();
        }
        let name = format!("desktop-{look}-settings");
        if want(&name) {
            main.global::<Store>().set_settings_open(true);
            render(&platform, idx, (1440, 900), 1.0, &out.join(format!("{name}.png")));
            main.global::<Store>().set_settings_open(false);
        }
    }
    a.invoke_set_look(0);

    // Overlay / widget
    let overlay = OverlayWindow::new().unwrap();
    app.attach_overlay(&overlay);
    let oidx = platform.windows.borrow().len() - 1;
    overlay.show().unwrap();
    for (li, look) in looks.iter().enumerate() {
        a.invoke_set_look(li as i32);
        let name = format!("overlay-{look}-panel");
        if want(&name) {
            overlay.global::<Store>().set_overlay_expanded(true);
            render(&platform, oidx, (340 * 2, 500 * 2), 2.0, &out.join(format!("{name}.png")));
        }
        let name = format!("overlay-{look}-bubble");
        if want(&name) {
            overlay.global::<Store>().set_overlay_expanded(false);
            render(&platform, oidx, (120, 120), 2.0, &out.join(format!("{name}.png")));
            overlay.global::<Store>().set_overlay_expanded(true);
        }
    }

    // Phone
    let pdir = std::env::temp_dir().join(format!("patch-shots-phone-{}", std::process::id()));
    std::fs::create_dir_all(&pdir).unwrap();
    seed(&pdir);
    let papp = App::new(pdir.clone(), Rc::new(DemoHost { mobile: true }));
    let phone = MainWindow::new().unwrap();
    papp.attach_main(&phone);
    papp.set_safe_area(24.0, 16.0);
    let pidx = platform.windows.borrow().len() - 1;
    phone.show().unwrap();
    let pa = phone.global::<Actions>();
    let scale = 2.625;
    let psize = ((412.0 * scale) as u32, (915.0 * scale) as u32);
    for (li, look) in looks.iter().enumerate() {
        pa.invoke_set_look(li as i32);
        for (vi, view) in ["day", "week", "month", "board", "list"].iter().enumerate() {
            let name = format!("phone-{look}-{view}");
            if want(&name) {
                pa.invoke_set_view(vi as i32);
                render(&platform, pidx, psize, scale, &out.join(format!("{name}.png")));
            }
        }
        let name = format!("phone-{look}-editor");
        if want(&name) {
            let id = app_task(&pdir, "Design review");
            pa.invoke_open_task(id.into());
            render(&platform, pidx, psize, scale, &out.join(format!("{name}.png")));
            pa.invoke_editor_close();
        }
        let name = format!("phone-{look}-settings");
        if want(&name) {
            phone.global::<Store>().set_settings_open(true);
            render(&platform, pidx, psize, scale, &out.join(format!("{name}.png")));
            phone.global::<Store>().set_settings_open(false);
        }
    }
    drop(app);
    drop(papp);
    println!("wrote screenshots to {}", out.display());
}

fn app_task(dir: &Path, title: &str) -> String {
    Doc::load_or_new(&dir.join("doc.json"))
        .tasks()
        .into_iter()
        .find(|t| t.title == title)
        .map(|t| t.id)
        .unwrap_or_default()
}
