//! Writes the pet's frames as upscaled PNGs: a contact sheet for review, and
//! with `--android <res dir>` the home-screen widget's pet drawables.

use patch_ui::pet::{frame, Mood, H, W};
use std::path::Path;

fn write(path: &Path, px: &[[u8; 4]], scale: usize, cols: usize, rows: usize) {
    let (w, h) = (W * cols * scale, H * rows * scale);
    let mut out = vec![0u8; w * h * 4];
    let frames = px.chunks(W * H).collect::<Vec<_>>();
    for (i, f) in frames.iter().enumerate() {
        let (fx, fy) = (i % cols * W * scale, i / cols * H * scale);
        for y in 0..H * scale {
            for x in 0..W * scale {
                let p = f[(y / scale) * W + x / scale];
                let o = ((fy + y) * w + fx + x) * 4;
                out[o..o + 4].copy_from_slice(&p);
            }
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let moods = [(Mood::Happy, "happy"), (Mood::Busy, "busy"), (Mood::Worried, "worried"), (Mood::Sleepy, "sleepy")];
    if args.first().map(String::as_str) == Some("--android") {
        let dir = Path::new(&args[1]).join("drawable-nodpi");
        std::fs::create_dir_all(&dir).unwrap();
        for (m, name) in moods {
            write(&dir.join(format!("pet_{name}.png")), &frame(m, 0, false, 0), 8, 1, 1);
        }
        return;
    }
    let out = args.first().map(String::as_str).unwrap_or("pet-sheet.png");
    let mut px = Vec::new();
    for (m, _) in moods {
        for tick in 0..4 {
            px.extend(frame(m, tick, tick == 3, 0));
        }
    }
    for skin in 0..patch_ui::pet::SKINS.len() {
        for (m, _) in moods {
            px.extend(frame(m, 0, false, skin));
        }
    }
    write(Path::new(out), &px, 10, 4, moods.len() + patch_ui::pet::SKINS.len());
}
