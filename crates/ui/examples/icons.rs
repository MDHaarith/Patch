// Regenerate all app icons: cargo run -p patch-ui --example icons
use patch_ui::pet::{frame, Mood, H, W};
use std::{fmt::Write, path::Path};

fn write_png(path: &Path, size: usize, pixels: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size as u32, size as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(pixels).unwrap();
}

fn render(pet: &[[u8; 4]], w: usize, h: usize, size: usize, adaptive: bool) -> Vec<u8> {
    let mut out = vec![0; size * size * 4];
    if !adaptive {
        let inset = size as f64 * 0.06;
        let radius = size as f64 * 0.20;
        for y in 0..size {
            for x in 0..size {
                let mut coverage = 0;
                // Antialias only the rounded background; the pet keeps hard pixel edges.
                for sy in 0..4 {
                    for sx in 0..4 {
                        let dx = (x as f64 + (sx as f64 + 0.5) / 4.0 - size as f64 / 2.0).abs();
                        let dy = (y as f64 + (sy as f64 + 0.5) / 4.0 - size as f64 / 2.0).abs();
                        let corner = size as f64 / 2.0 - inset - radius;
                        if (dx - corner).max(0.0).powi(2) + (dy - corner).max(0.0).powi(2) <= radius.powi(2) {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 {
                    continue;
                }
                let shade = 1.0 - y as f64 / (size - 1) as f64;
                let gray = (14.0 + 61.0 * shade).round() as u8;
                let i = (y * size + x) * 4;
                out[i..i + 4].copy_from_slice(&[gray, gray, gray + 2, (coverage * 255 / 16) as u8]);
            }
        }
    }
    let fraction = if adaptive { 0.60 } else { 0.70 };
    let scale = size as f64 * fraction / w as f64;
    let scale = if adaptive { scale.floor() } else { scale.round() }.max(1.0) as usize;
    // shortcut: 16px cannot fit the native 18px pet; use nearest-neighbour until larger icons are possible.
    let width = if size < w { (size as f64 * fraction).round() as usize } else { w * scale };
    let height = if size < w { h * width / w } else { h * scale };
    let (left, top) = ((size - width) / 2, (size - height) / 2);
    for y in 0..height {
        for x in 0..width {
            let p = pet[y * h / height * w + x * w / width];
            if p[3] == 0 {
                continue;
            }
            if adaptive {
                // Every pixel corner must fit inside the adaptive icon's 66% safe circle.
                let dx = ((left + x) as f64 + 0.5 - size as f64 / 2.0).abs() + 0.5;
                let dy = ((top + y) as f64 + 0.5 - size as f64 / 2.0).abs() + 0.5;
                assert!(dx.hypot(dy) <= size as f64 * 0.33);
            }
            let i = ((top + y) * size + left + x) * 4;
            out[i..i + 4].copy_from_slice(&p);
        }
    }
    assert_eq!(out[3], 0, "corners stay transparent");
    out
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Busy is the neutral idle face with open eyes; Happy has closed, smiling eyes.
    let pixels = frame(Mood::Busy, 0, false, 0);
    let occupied: Vec<_> = (0..W * H).filter(|&i| pixels[i][3] != 0).collect();
    let left = occupied.iter().map(|i| i % W).min().unwrap();
    let top = occupied.iter().map(|i| i / W).min().unwrap();
    let w = occupied.iter().map(|i| i % W).max().unwrap() - left + 1;
    let h = occupied.iter().map(|i| i / W).max().unwrap() - top + 1;
    let pet: Vec<_> = (top..top + h).flat_map(|y| pixels[y * W + left..y * W + left + w].iter().copied()).collect();

    for (name, size) in [("icon.png", 256), ("icon-1024.png", 1024)] {
        write_png(&root.join("assets/images").join(name), size, &render(&pet, w, h, size, false));
    }
    for size in [16, 22, 24, 32, 48, 64, 128, 256, 512] {
        let path = root.join(format!("packaging/linux/icons/hicolor/{size}x{size}/apps/com.mdhaarith.Patch.png"));
        write_png(&path, size, &render(&pet, w, h, size, false));
    }
    let res = root.join("android/app/src/main/res");
    for (density, size) in [("mdpi", 108), ("hdpi", 162), ("xhdpi", 216), ("xxhdpi", 324), ("xxxhdpi", 432)] {
        write_png(
            &res.join(format!("mipmap-{density}/ic_launcher_foreground.png")),
            size,
            &render(&pet, w, h, size, true),
        );
    }
    for (name, size, scale) in
        [("ic_launcher_monochrome", 108, (108.0 * 0.60 / w as f64).floor() as usize), ("ic_stat_patch", 24, 1)]
    {
        let (left, top) = ((size - w * scale) / 2, (size - h * scale) / 2);
        let mut path = String::new();
        for (i, p) in pet.iter().enumerate() {
            if p[3] != 0 {
                let (x, y) = (left + i % w * scale, top + i / w * scale);
                write!(path, "M{x},{y}h{scale}v{scale}h-{scale}z").unwrap();
            }
        }
        std::fs::write(
            res.join(format!("drawable/{name}.xml")),
            format!(
                r##"<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="{size}dp"
    android:height="{size}dp"
    android:viewportWidth="{size}"
    android:viewportHeight="{size}">
    <path android:fillColor="#FFFFFFFF" android:pathData="{path}" />
</vector>
"##
            ),
        )
        .unwrap();
    }
    println!("Regenerated desktop and Android icons from pet::frame(Mood::Busy, 0, false)");
}
