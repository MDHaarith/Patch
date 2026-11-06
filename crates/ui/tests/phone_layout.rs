//! Phone-sized software renders, also usable for visual QA with PATCH_LAYOUT_PNG_DIR.
use patch_ui::{EnvironmentRow, MainWindow, Store, Theme};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, VecModel};
use std::cell::Cell;
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

fn snapshot(name: &str, pixels: &[PremultipliedRgbaColor], width: u32, height: u32) {
    let Some(dir) = std::env::var_os("PATCH_LAYOUT_PNG_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let file =
        std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = pixels
        .iter()
        .flat_map(|p| [p.red, p.green, p.blue, p.alpha])
        .collect();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&bytes)
        .unwrap();
}

#[test]
fn phone_cards_and_controls() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let time = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform(window.clone(), time.clone()))).unwrap();
    let main = MainWindow::new().unwrap();
    let store = main.global::<Store>();
    store.set_mobile(true);
    store.set_settings_open(true);
    store.set_battery_state(1);
    store.set_sync_label("Offline — can't find mdhlaptop".into());
    store.set_stat_week(Rc::new(VecModel::from(vec![0, 0, 0, 5, 1, 0, 1])).into());
    store.set_stat_week_max(5);
    store.set_environments(
        Rc::new(VecModel::from(vec![
            EnvironmentRow {
                name: "MDHLaptop".into(),
                url: "http://mdhlaptop:4747".into(),
                status: "Offline — can't find mdhlaptop".into(),
                enabled: true,
                connected: false,
            },
            EnvironmentRow {
                name: "MDHLaptop".into(),
                url: "http://100.101.102.103:4747".into(),
                status: "Sync off".into(),
                enabled: false,
                connected: false,
            },
        ]))
        .into(),
    );
    main.show().unwrap();
    window.set_size(PhysicalSize::new(412, 915));
    let render = || {
        time.set(time.get() + Duration::from_millis(300));
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let mut pixels = vec![PremultipliedRgbaColor::default(); 412 * 915];
        window.draw_if_needed(|r| {
            r.render(&mut pixels, 412);
        });
        pixels
    };
    render();
    let pixels = render();
    snapshot("settings-phone", &pixels, 412, 915);
    // All nonzero days use the same narrow width and share the baseline.
    let blue = |p: &PremultipliedRgbaColor| p.blue > 200 && p.red < 30 && p.green > 100;
    let columns: Vec<_> = (32..380)
        .filter(|&x| (155..220).any(|y| blue(&pixels[y * 412 + x])))
        .collect();
    let mut widths = Vec::new();
    for x in &columns {
        if *x == 0 || !columns.contains(&(x - 1)) {
            widths.push(0);
        }
        *widths.last_mut().unwrap() += 1;
    }
    assert_eq!(widths, vec![16, 16, 16]);
    let baselines: Vec<_> = [206, 257, 358]
        .map(|x| (155..225).rfind(|&y| blue(&pixels[y * 412 + x])).unwrap())
        .into();
    assert!(baselines.iter().all(|y| *y == baselines[0]));
    window.dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(200.0, 500.0),
        delta_x: 0.0,
        delta_y: -700.0,
    });
    let pixels = render();
    snapshot("settings-phone-lower", &pixels, 412, 915);
    // A lingering touch hover over Years must not look like a second selection.
    window.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(280.0, 175.0),
    });
    let pixels = render();
    let rgb = |p: &PremultipliedRgbaColor| (p.red, p.green, p.blue);
    assert_eq!(rgb(&pixels[165 * 412 + 280]), rgb(&pixels[165 * 412 + 45]));
    store.set_environment_page(1);
    let pixels = render();
    snapshot("environments-phone", &pixels, 412, 915);
    assert_environment_card(&pixels);
    main.global::<Theme>().set_text_scale(1.3);
    let pixels = render();
    snapshot("environments-phone-large", &pixels, 412, 915);
    assert_environment_card(&pixels);

    // Details must also appear when a newly typed address hasn't been normalized.
    main.global::<Theme>().set_text_scale(1.0);
    store.set_environment_page(2);
    store.set_hub_url("http://mdhlaptop:4747".into());
    store.set_sync_state(3);
    let before = render();
    store.set_sync_detail("Offline — can't reach hub: mdhlaptop: failed to lookup address information: No address associated with hostname".into());
    let after = render();
    assert!(
        before.iter().zip(&after).any(|(a, b)| rgb(a) != rgb(b)),
        "connection page shows the full error"
    );
    snapshot("environment-connection-error", &after, 412, 915);
}

fn assert_environment_card(pixels: &[PremultipliedRgbaColor]) {
    let card_rows: Vec<_> = (80..915)
        .filter(|&y| pixels[y * 412 + 206].red > 0)
        .collect();
    assert!(
        *card_rows.first().unwrap() < 130,
        "card starts below the header"
    );
    let bottom = *card_rows.last().unwrap();
    let last_ink = (100..915)
        .rfind(|&y| (70..275).any(|x| pixels[y * 412 + x].red > 65))
        .unwrap();
    assert!(
        last_ink + 12 < bottom,
        "wrapped text stays inside the card with bottom padding: ink={last_ink}, bottom={bottom}"
    );
}
