use patch_ui::{OverlayWindow, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    time: Rc<Cell<Duration>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.time.get()
    }
}

#[test]
fn both_directions_use_the_current_pet_after_screen_and_keyboard_clamping() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let time = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform { window: window.clone(), time: time.clone() })).unwrap();
    let overlay = OverlayWindow::new().unwrap();
    overlay.global::<Store>().set_mobile(true);
    overlay.global::<Store>().set_overlay_expanded(false);
    overlay.show().unwrap();
    let render = |ms, height| {
        time.set(time.get() + Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
        window.set_size(PhysicalSize::new(340, height));
        let mut pixels = vec![PremultipliedRgbaColor::default(); 340 * height as usize];
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(&mut pixels, 340);
        });
        pixels
    };
    for (x, y, height, far_y, near_y) in [
        (4, 4, 500, 400, 65),
        (276, 4, 500, 400, 65),
        (4, 436, 500, 100, 435),
        (276, 436, 500, 100, 435),
        (140, 220, 500, 450, 281),
        (276, 436, 300, 50, 239), // The IME clamps the pet to y=240.
    ] {
        overlay.set_pet_x(x as f32);
        overlay.set_pet_y(y as f32);
        render(300, height);
        for expanded in [true, false] {
            overlay.global::<Store>().set_overlay_expanded(expanded);
            render(0, height);
            let pixels = render(if expanded { 40 } else { 110 }, height);
            let backdrop = pixels[339].alpha;
            assert!(backdrop > 0, "sample during the animation");
            assert_eq!(
                pixels[far_y * 340 + 170].alpha,
                backdrop,
                "panel travels toward the pet: expanded={expanded}, pet=({x},{y})"
            );
            assert!(
                pixels[near_y * 340 + x + 30].alpha > backdrop,
                "panel remains next to the pet: expanded={expanded}, pet=({x},{y})"
            );
            render(300, height);
        }
    }
}
