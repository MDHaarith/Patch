//! `patch --screenshot DIR`: renders the main window in each look and the
//! widget (panel and bubble) to PNG files with `Window::take_snapshot`, for
//! CI and store listings. Restores the look and widget size afterwards.

use crate::shell::Shell;
use patch_ui::{Actions, Store};
use slint::{ComponentHandle, LogicalSize, Rgba8Pixel, SharedPixelBuffer, Timer};
use std::cell::Cell;
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

const LOOKS: [(i32, &str); 3] = [(0, "matte"), (1, "glass"), (2, "light")];

/// One step of the script; returns how long to let the UI settle after it.
type Step = Box<dyn FnOnce(&Rc<Shell>) -> Duration>;

pub fn write_png(path: &Path, image: &SharedPixelBuffer<Rgba8Pixel>) -> io::Result<()> {
    let file = io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(image.as_bytes()).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)
}

fn capture(window: &slint::Window, path: PathBuf) {
    match window.take_snapshot().map_err(|e| e.to_string()).and_then(|img| {
        write_png(&path, &img).map_err(|e| e.to_string())?;
        Ok((img.width(), img.height()))
    }) {
        Ok((w, h)) => println!("{} ({w}×{h})", path.display()),
        Err(e) => eprintln!("patch: can't capture {}: {e}", path.display()),
    }
}

pub fn start(shell: &Rc<Shell>, dir: PathBuf) -> io::Result<()> {
    std::fs::create_dir_all(&dir)?;
    const SETTLE: Duration = Duration::from_millis(700);
    let mut steps: VecDeque<Step> = VecDeque::new();
    // (look, widget expanded) to restore at the end.
    let original = Rc::new(Cell::new((0, true)));

    let o = original.clone();
    steps.push_back(Box::new(move |s| {
        if let Some(main) = s.main() {
            o.set((s.look(), main.global::<Store>().get_overlay_expanded()));
        }
        s.show_main_sized(LogicalSize::new(1320.0, 840.0));
        // The widget without touching the saved preference.
        patch_ui::Host::set_widget(&**s, true);
        Duration::from_millis(1500)
    }));
    steps.push_back(Box::new(|s| {
        if let Some(w) = s.widget_window() {
            w.global::<Actions>().invoke_overlay_expand(true);
        }
        SETTLE
    }));
    for (look, name) in LOOKS {
        steps.push_back(Box::new(move |s| {
            if let Some(main) = s.main() {
                main.global::<Actions>().invoke_set_look(look);
            }
            SETTLE
        }));
        let dir = dir.clone();
        steps.push_back(Box::new(move |s| {
            if let Some(main) = s.main() {
                capture(main.window(), dir.join(format!("main-{name}.png")));
            }
            if let Some(w) = s.widget_window() {
                capture(w.window(), dir.join(format!("widget-{name}.png")));
            }
            Duration::ZERO
        }));
    }
    // The bubble in the user's own look, then everything back as it was.
    let o = original.clone();
    steps.push_back(Box::new(move |s| {
        if let Some(main) = s.main() {
            main.global::<Actions>().invoke_set_look(o.get().0);
        }
        if let Some(w) = s.widget_window() {
            w.global::<Actions>().invoke_overlay_expand(false);
        }
        SETTLE
    }));
    let d = dir.clone();
    steps.push_back(Box::new(move |s| {
        if let Some(w) = s.widget_window() {
            capture(w.window(), d.join("bubble.png"));
            w.global::<Actions>().invoke_overlay_expand(original.get().1);
        }
        Duration::from_millis(100)
    }));
    steps.push_back(Box::new(|s| {
        s.quit();
        Duration::ZERO
    }));
    run(Rc::downgrade(shell), steps);
    Ok(())
}

fn run(shell: Weak<Shell>, mut steps: VecDeque<Step>) {
    let (Some(step), Some(s)) = (steps.pop_front(), shell.upgrade()) else { return };
    let wait = step(&s);
    Timer::single_shot(wait, move || run(shell, steps));
}
