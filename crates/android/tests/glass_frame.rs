//! Complete software-renderer cost, including backdrop capture and final paint.
use patch_android::buffer::BackBuffer;
use patch_android::window::SurfaceWindow;
use patch_ui::{Block, MainWindow, Store, Theme};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model, PhysicalSize};
use std::rc::Rc;
use std::time::Instant;

struct TestPlatform(Rc<SurfaceWindow>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn dock_capture_follows_layout_and_reuses_output() {
    let adapter = SurfaceWindow::new();
    slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
    let main = MainWindow::new().unwrap();
    let store = main.global::<Store>();
    store.set_mobile(true);
    store.set_view(0);
    store.set_liquid_supported(true);
    main.global::<Theme>().set_glass(true);
    let blocks = Rc::new(slint::VecModel::from(vec![Block {
        span: 1440.0,
        lane_w: 1.0,
        ..Default::default()
    }]));
    store.set_grid_blocks(blocks.clone().into());
    main.show().unwrap();
    main.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 3.0 });
    main.window().set_size(PhysicalSize::new(1080, 2400));
    let mut buffer = BackBuffer::default();
    buffer.resize(1080, 2400);
    adapter.draw_main(&mut buffer, true, &main);
    let pointer = || {
        store
            .get_dock_glass()
            .to_rgba8()
            .unwrap()
            .as_slice()
            .as_ptr()
    };
    let first = pointer();
    let change = |i: u8| {
        let mut block = blocks.row_data(0).unwrap();
        block.color = slint::Color::from_rgb_u8(i, 90, 160);
        blocks.set_row_data(0, block);
        slint::platform::update_timers_and_animations();
    };
    change(40);
    adapter.draw_main(&mut buffer, false, &main);
    let second = pointer();
    change(50);
    adapter.draw_main(&mut buffer, false, &main);
    assert_eq!(
        first,
        pointer(),
        "reuse the first output after Slint releases it"
    );
    change(55);
    adapter.draw_main(&mut buffer, false, &main);
    assert_eq!(second, pointer(), "reuse the second output too");
    let rect = store.get_dock_rect();
    assert_eq!(
        store.get_dock_glass().size().width,
        rect.width.ceil() as u32
    );
    store.set_safe_bottom(250.0);
    adapter.draw_main(&mut buffer, false, &main);
    let moved = store.get_dock_rect();
    assert_eq!(
        rect.y - moved.y,
        250.0,
        "use the dock's actual IME-shifted position"
    );
    store.set_ime_animating(true);
    change(60);
    let frozen = pointer();
    adapter.draw_main(&mut buffer, false, &main);
    assert_eq!(frozen, pointer(), "IME animation must not recapture");
    store.set_ime_animating(false);
    store.set_safe_bottom(0.0);
    adapter.draw_main(&mut buffer, false, &main);
    assert!(
        adapter.draw_main(&mut buffer, false, &main).is_none(),
        "idle frames must not capture again"
    );
    if cfg!(debug_assertions) {
        return;
    }
    // Run in release mode with --nocapture for useful timing; both paths paint
    // the same changing full-height event beneath the dock, without resizing.
    let mut times = [0.0; 2];
    for (capture, ms) in times.iter_mut().enumerate() {
        for i in 0..110 {
            change(if i % 2 == 0 { 20 } else { 220 });
            let start = Instant::now();
            let dirty = if capture == 0 {
                adapter.draw(&mut buffer, false)
            } else {
                adapter.draw_main(&mut buffer, false, &main)
            }
            .expect("event dirties the backdrop");
            if i >= 10 {
                *ms += start.elapsed().as_secs_f64() * 10.0;
            }
            assert!(dirty.bottom() > (store.get_dock_rect().y * 3.0) as u32);
        }
    }
    eprintln!(
        "1080x2400 scale 3: draw {:.3} ms, draw_main {:.3} ms, extra {:.3} ms/frame",
        times[0],
        times[1],
        times[1] - times[0]
    );
}
