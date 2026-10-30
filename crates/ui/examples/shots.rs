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
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct ShotPlatform {
    windows: RefCell<Vec<Rc<MinimalSoftwareWindow>>>,
    time: Cell<std::time::Duration>,
}

impl Platform for ShotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(w.clone());
        Ok(w)
    }

    fn duration_since_start(&self) -> std::time::Duration {
        self.time.get()
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
    fn set_hosting(&self, on: bool) -> Result<Option<patch_ui::HostedHub>, String> {
        Ok(on.then(|| patch_ui::HostedHub {
            local_url: "http://127.0.0.1:1".into(), urls: vec!["http://100.64.0.2:4747".into()], token: "K7QF-2MZD-9XPA-T4RH".into(),
        }))
    }
    fn devices(&self) -> Vec<patch_sync::Device> {
        ["demo", "phone"].into_iter().map(|id| patch_sync::Device {
            id: id.into(), name: if id == "demo" { "Laptop".into() } else { "Pixel 9".into() },
            platform: if id == "demo" { "linux".into() } else { "android".into() },
            paired_at: 0, last_seen: patch_core::hlc::now_ms(), address: "100.64.0.3".into(), revoked: false,
        }).collect()
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

fn frame(platform: &ShotPlatform, index: usize, size: (u32, u32), scale: f32) -> Vec<PremultipliedRgbaColor> {
    let w = platform.windows.borrow()[index].clone();
    w.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    w.set_size(PhysicalSize::new(size.0, size.1));
    let mut buf = vec![PremultipliedRgbaColor::default(); (size.0 * size.1) as usize];
    w.request_redraw();
    w.draw_if_needed(|r| { r.render(&mut buf, size.0 as usize); });
    buf
}

// Advance in frames so timer-driven transitions also evaluate their bindings.
fn advance(platform: &ShotPlatform, index: usize, size: (u32, u32), scale: f32, ms: u64) {
    let mut remaining = ms;
    while remaining > 0 {
        let step = remaining.min(10);
        platform.time.set(platform.time.get() + std::time::Duration::from_millis(step));
        slint::platform::update_timers_and_animations();
        frame(platform, index, size, scale);
        remaining -= step;
    }
}

fn render(platform: &ShotPlatform, index: usize, size: (u32, u32), scale: f32, out: &Path) {
    frame(platform, index, size, scale);
    advance(platform, index, size, scale, 420);
    capture(platform, index, size, scale, out);
}

fn capture(platform: &ShotPlatform, index: usize, size: (u32, u32), scale: f32, out: &Path) {
    let buf = frame(platform, index, size, scale);
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
    let platform = Rc::new(ShotPlatform { windows: RefCell::new(Vec::new()), time: Cell::new(std::time::Duration::ZERO) });
    struct Shared(Rc<ShotPlatform>);
    impl Platform for Shared {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            self.0.create_window_adapter()
        }
        fn duration_since_start(&self) -> std::time::Duration {
            self.0.duration_since_start()
        }
    }
    slint::platform::set_platform(Box::new(Shared(platform.clone()))).unwrap();
    let only = std::env::var("SHOTS").unwrap_or_default();
    if only == "motion" {
        motion(&platform, &out);
        return;
    }
    let want = |name: &str| only.is_empty() || only.split(',').any(|o| name.starts_with(o));
    let looks = ["matte", "light"];

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
        let name = format!("desktop-{look}-pairing");
        if want(&name) {
            a.invoke_set_hosting(true);
            main.global::<Store>().set_settings_open(true);
            let path = out.join(format!("{name}.png"));
            render(&platform, idx, (1440, 1100), 1.0, &path);
            platform.windows.borrow()[idx].dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700.0, 500.0), delta_x: 0.0, delta_y: -720.0,
            });
            render(&platform, idx, (1440, 1100), 1.0, &path);
            main.global::<Store>().set_settings_open(false);
            a.invoke_set_hosting(false);
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
            overlay.global::<Store>().set_overlay_tab(0);
            render(&platform, oidx, (340 * 2, 500 * 2), 2.0, &out.join(format!("{name}.png")));
        }
        let name = format!("overlay-{look}-panel-calendar");
        if want(&name) {
            overlay.global::<Store>().set_overlay_expanded(true);
            overlay.global::<Store>().set_overlay_tab(3);
            render(&platform, oidx, (340 * 2, 500 * 2), 2.0, &out.join(format!("{name}.png")));
        }
        let name = format!("overlay-{look}-bubble");
        if want(&name) {
            overlay.global::<Store>().set_overlay_expanded(false);
            render(&platform, oidx, (120, 120), 2.0, &out.join(format!("{name}.png")));
            overlay.global::<Store>().set_overlay_expanded(true);
        }
    }

    let desk = OverlayWindow::new().unwrap();
    desk.set_mode(2);
    app.attach_desk(&desk);
    let didx = platform.windows.borrow().len() - 1;
    desk.show().unwrap();
    for (li, look) in looks.iter().enumerate() {
        let name = format!("desk-{look}");
        if want(&name) {
            a.invoke_set_look(li as i32);
            render(&platform, didx, (704, 704), 2.0, &out.join(format!("{name}.png")));
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
        for (page, suffix) in [(1, "environments"), (1, "environments-connected"), (2, "add-environment")] {
            let name = format!("phone-{look}-{suffix}");
            if want(&name) {
                let store = phone.global::<Store>();
                store.set_settings_open(true);
                store.set_environment_page(page);
                if suffix == "environments-connected" {
                    store.set_environments(Rc::new(slint::VecModel::from(vec![patch_ui::EnvironmentRow {
                        url: "http://laptop:4747".into(), name: "My laptop".into(), status: "Connected".into(), connected: true, enabled: true,
                    }])).into());
                }
                render(&platform, pidx, psize, scale, &out.join(format!("{name}.png")));
                store.set_environments(Rc::new(slint::VecModel::from(Vec::<patch_ui::EnvironmentRow>::new())).into());
                store.set_environment_page(0);
                store.set_settings_open(false);
            }
        }
        let name = format!("phone-{look}-settings");
        if want(&name) {
            phone.global::<Store>().set_settings_open(true);
            render(&platform, pidx, psize, scale, &out.join(format!("{name}.png")));
            phone.global::<Store>().set_settings_open(false);
        }
    }
    for (tab, name) in [(0, "phone-overlay-popup"), (3, "phone-overlay-popup-calendar")] {
        if want(name) {
            let pov = OverlayWindow::new().unwrap();
            papp.attach_overlay(&pov);
            let idx = platform.windows.borrow().len() - 1;
            pov.show().unwrap();
            pov.global::<Store>().set_overlay_expanded(true);
            pov.global::<Store>().set_overlay_tab(tab);
            render(&platform, idx, psize, scale, &out.join(format!("{name}.png")));
        }
    }
    drop(app);
    drop(papp);
    println!("wrote screenshots to {}", out.display());
}

// SHOTS=motion cargo run -p patch-ui --example shots -- /tmp/anim
fn motion(platform: &ShotPlatform, out: &Path) {
    use slint::platform::PointerEventButton;
    use slint::LogicalPosition;
    let dir = std::env::temp_dir().join(format!("patch-motion-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    seed(&dir);
    let app = App::new(dir.clone(), Rc::new(DemoHost { mobile: true }));
    let overlay = OverlayWindow::new().unwrap();
    app.attach_overlay(&overlay);
    let oi = platform.windows.borrow().len() - 1;
    overlay.set_pet_x(4.0);
    overlay.set_pet_y(4.0);
    overlay.global::<Store>().set_overlay_expanded(false);
    overlay.show().unwrap();
    let size = (412, 915);
    render(platform, oi, size, 1.0, &out.join("overlay-collapsed.png"));
    for (expanded, name, samples) in [
        (true, "open", [0, 37, 60, 107, 300]),
        (false, "close", [0, 89, 143, 186, 220]),
    ] {
        overlay.global::<Store>().set_overlay_expanded(expanded);
        frame(platform, oi, size, 1.0);
        let mut previous = 0;
        for (i, ms) in samples.into_iter().enumerate() {
            advance(platform, oi, size, 1.0, ms - previous);
            capture(platform, oi, size, 1.0, &out.join(format!("overlay-{name}-{i}.png")));
            previous = ms;
        }
        advance(platform, oi, size, 1.0, 100);
    }
    let phone = MainWindow::new().unwrap();
    app.attach_main(&phone);
    app.set_safe_area(24.0, 16.0);
    let pi = platform.windows.borrow().len() - 1;
    phone.show().unwrap();
    phone.global::<Actions>().invoke_set_view(2);
    render(platform, pi, size, 1.0, &out.join("month-idle.png"));
    let window = platform.windows.borrow()[pi].clone();
    let start = LogicalPosition::new(280.0, 450.0);
    window.dispatch_event(WindowEvent::PointerPressed { position: start, button: PointerEventButton::Left });
    for step in 1..=10 {
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(start.x - 16.0 * step as f32, start.y),
        });
        advance(platform, pi, size, 1.0, 16);
        if step == 5 {
            capture(platform, pi, size, 1.0, &out.join("month-mid-drag.png"));
        }
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(120.0, start.y), button: PointerEventButton::Left,
    });
    frame(platform, pi, size, 1.0);
    for (ms, name) in [(75, "mid-out"), (75, "snap"), (20, "enter-start"), (120, "mid-in"), (120, "settled")] {
        advance(platform, pi, size, 1.0, ms);
        capture(platform, pi, size, 1.0, &out.join(format!("month-{name}.png")));
    }
    std::fs::remove_dir_all(dir).unwrap();
    println!("wrote motion frames to {}", out.display());
}

fn app_task(dir: &Path, title: &str) -> String {
    Doc::load_or_new(&dir.join("doc.json"))
        .tasks()
        .into_iter()
        .find(|t| t.title == title)
        .map(|t| t.id)
        .unwrap_or_default()
}
