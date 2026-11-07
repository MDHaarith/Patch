//! Phone-sized software renders, also usable for visual QA with PATCH_LAYOUT_PNG_DIR.
use patch_ui::{Actions, EnvironmentRow, MainWindow, Store, Theme};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
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
    store.set_stat_weekdays(
        Rc::new(VecModel::from(
            ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
                .map(Into::into)
                .to_vec(),
        ))
        .into(),
    );
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
    // The repeat and alert builders use full-width, equally tall Add rows.
    let add_rows =
        runs((60..915).filter(|&y| blue(&pixels[y * 412 + 100]) && blue(&pixels[y * 412 + 310])));
    assert_eq!(add_rows.len(), 2, "one Add row per builder");
    let repeat_add = add_rows[0].0;
    let alert_add = add_rows[1].0;
    assert_eq!(add_rows[0].1 - repeat_add, 39, "40px Add button");
    assert_eq!(add_rows[1].1 - alert_add, 39);
    let repeat_calls = Rc::new(Cell::new((0, 0)));
    let alert_call = Rc::new(Cell::new(0));
    let archive_call = Rc::new(Cell::new(-1));
    main.global::<Actions>().on_add_custom_repeat({
        let calls = repeat_calls.clone();
        move |n, unit| calls.set((n, unit))
    });
    main.global::<Actions>().on_add_custom_alert({
        let call = alert_call.clone();
        move |n| call.set(n)
    });
    main.global::<Actions>().on_set_archive_days({
        let call = archive_call.clone();
        move |n| call.set(n)
    });
    // Every / Before share the same stepper columns. The completion option is
    // immediately before Add repeat and is passed only to that repeat.
    click(&window, 185.0, (repeat_add - 132) as f32);
    click(&window, 350.0, (repeat_add - 34) as f32);
    click(&window, 100.0, (repeat_add + 20) as f32);
    assert_eq!(repeat_calls.get(), (3, 5));
    click(&window, 185.0, (alert_add - 74) as f32);
    click(&window, 100.0, (alert_add + 20) as f32);
    assert_eq!(alert_call.get(), 35);
    // Archive is after both builders; its current value starts at zero.
    click(&window, 185.0, (alert_add + 103) as f32);
    assert_eq!(archive_call.get(), 1);
    assert_equal_segments(&window, &render, repeat_add - 104, 4);
    assert_equal_segments(&window, &render, alert_add - 46, 3);
    let rgb = |p: &PremultipliedRgbaColor| (p.red, p.green, p.blue);
    store.set_environment_page(1);
    let pixels = render();
    snapshot("environments-phone", &pixels, 412, 915);
    assert_environment_card(&pixels);
    assert_header_gutters(&pixels);
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

fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn runs(values: impl Iterator<Item = usize>) -> Vec<(usize, usize)> {
    let mut result: Vec<(usize, usize)> = Vec::new();
    for value in values {
        if let Some(last) = result.last_mut() {
            if last.1 + 1 == value {
                last.1 = value;
                continue;
            }
        }
        result.push((value, value));
    }
    result
}

fn assert_equal_segments(
    window: &MinimalSoftwareWindow,
    render: &impl Fn() -> Vec<PremultipliedRgbaColor>,
    top: usize,
    count: usize,
) {
    let mut widths = Vec::new();
    for i in 0..count {
        click(
            window,
            32.0 + (i as f32 + 0.5) * 348.0 / count as f32,
            (top + 18) as f32,
        );
        let pixels = render();
        let selected = runs((32..380).filter(|&x| {
            let p = pixels[(top + 12) * 412 + x];
            p.red == 46 && p.green == 46 && p.blue == 46
        }));
        assert_eq!(
            selected.len(),
            1,
            "only one selected pill (including touch hover)"
        );
        let (left, right) = selected[0];
        widths.push(right - left + 1);
        if i == 0 {
            assert_eq!(left, 34, "2px left inset");
        }
        if i + 1 == count {
            assert_eq!(right, 377, "2px right inset");
        }
    }
    assert!(
        widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1,
        "equal segment widths: {widths:?}"
    );
}

fn assert_header_gutters(pixels: &[PremultipliedRgbaColor]) {
    let ink = |x: usize, y: usize| pixels[y * 412 + x].red > 100;
    let back = (0..40).find(|&x| (0..45).any(|y| ink(x, y))).unwrap();
    let title = (0..40).find(|&x| (48..80).any(|y| ink(x, y))).unwrap();
    let right = (370..412).rfind(|&x| (0..45).any(|y| ink(x, y))).unwrap();
    assert!(
        back.abs_diff(title) <= 3,
        "back icon and title share the gutter: {back}, {title}"
    );
    assert!(
        back.abs_diff(411 - right) <= 3,
        "left and right icons have equal gutters"
    );
}
