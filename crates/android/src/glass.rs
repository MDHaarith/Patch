//! Bounded CPU backdrop filter, sampled at roughly one pixel per logical
//! pixel. The caller hides the entire dock, so controls never refract.
use crate::buffer::Rect;
use slint::platform::software_renderer::PremultipliedRgbaColor as Pixel;
use slint::{Rgba8Pixel, SharedPixelBuffer};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub blur: f32,
    pub refraction: f32,
    pub chromatic: bool,
    pub depth: bool,
    pub tint: [u8; 3],
    pub opacity: f32,
    pub scale: f32,
}

#[derive(Clone, Copy, Default)]
struct Lens {
    red: usize,
    green: usize,
    blue: usize,
    alpha: u8,
    shade: f32,
}

#[derive(Default)]
pub struct Glass {
    a: Vec<Pixel>,
    source: Vec<Pixel>,
    last: Option<(Rect, Settings, usize, usize)>,
    b: Vec<Pixel>,
    image: Option<SharedPixelBuffer<Rgba8Pixel>>,
    spare: Option<SharedPixelBuffer<Rgba8Pixel>>,
    lens: Vec<Lens>,
}

impl Glass {
    /// All coordinates are physical pixels. Samples outside the screen clamp
    /// to its edge. Scratch storage grows only when the dock grows.
    pub fn render(
        &mut self,
        source: &[Pixel],
        width: u32,
        height: u32,
        rect: Rect,
        settings: Settings,
    ) -> SharedPixelBuffer<Rgba8Pixel> {
        let sample = settings.scale.round().clamp(1.0, 4.0) as u32;
        let ow = rect.w.div_ceil(sample).max(1) as usize;
        let oh = rect.h.div_ceil(sample).max(1) as usize;
        let radius =
            (settings.blur.clamp(0.0, 20.0) * settings.scale / sample as f32).round() as usize;
        let margin = radius * 2;
        let w = ow + margin * 2;
        let h = oh + margin * 2;
        let geometry_changed = self.last != Some((rect, settings, w, h));
        let mut changed = geometry_changed;
        self.source.resize(w * h, Pixel::default());
        self.a.resize(w * h, Pixel::default());
        self.b.resize(w * h, Pixel::default());
        for y in 0..h {
            let sy = (rect.y as i64 + (y as i64 - margin as i64) * sample as i64)
                .clamp(0, height as i64 - 1) as usize;
            for x in 0..w {
                let sx = (rect.x as i64 + (x as i64 - margin as i64) * sample as i64)
                    .clamp(0, width as i64 - 1) as usize;
                let pixel = source[sy * width as usize + sx];
                let old = &mut self.source[y * w + x];
                changed |= [old.red, old.green, old.blue, old.alpha]
                    != [pixel.red, pixel.green, pixel.blue, pixel.alpha];
                *old = pixel;
            }
        }
        if !changed {
            return self.image.as_ref().unwrap().clone();
        }
        self.last = Some((rect, settings, w, h));
        self.a.copy_from_slice(&self.source);
        if radius > 0 {
            for _ in 0..2 {
                blur(&self.a, &mut self.b, w, h, radius, false);
                blur(&self.b, &mut self.a, w, h, radius, true);
            }
        }
        // Slint retains the last displayed image even when the dock is hidden.
        // Alternate outputs so the next write owns its storage exclusively.
        std::mem::swap(&mut self.image, &mut self.spare);
        let image = self
            .image
            .get_or_insert_with(|| SharedPixelBuffer::new(ow as u32, oh as u32));
        if image.width() != ow as u32 || image.height() != oh as u32 {
            *image = SharedPixelBuffer::new(ow as u32, oh as u32);
        }
        let pixels = image.make_mut_slice();
        if geometry_changed {
            self.lens.resize(ow * oh, Lens::default());
            let corner = (30.0 * settings.scale / sample as f32).min(oh as f32 / 2.0);
            let band = (corner * settings.refraction.clamp(0.0, 1.0)).max(0.5);
            for y in 0..oh {
                for x in 0..ow {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let cx = px.clamp(corner, ow as f32 - corner);
                    let cy = py.clamp(corner, oh as f32 - corner);
                    let dx = px - cx;
                    let dy = py - cy;
                    let len = (dx * dx + dy * dy).sqrt();
                    let distance = if len > 0.0 {
                        corner - len
                    } else {
                        px.min(ow as f32 - px).min(py).min(oh as f32 - py)
                    };
                    let coverage = (distance + 0.5).clamp(0.0, 1.0);
                    let edge = (1.0 - distance.max(0.0) / band).clamp(0.0, 1.0);
                    let bend = edge * edge * (3.0 - 2.0 * edge) * band * 0.6 * settings.refraction;
                    let (nx, ny) = if len > 0.0 {
                        (-dx / len, -dy / len)
                    } else if py.min(oh as f32 - py) < px.min(ow as f32 - px) {
                        (0.0, if py < oh as f32 / 2.0 { 1.0 } else { -1.0 })
                    } else {
                        (if px < ow as f32 / 2.0 { 1.0 } else { -1.0 }, 0.0)
                    };
                    let sample = |factor: f32| {
                        let sx = (x as f32 + margin as f32 + nx * bend * factor + 0.5)
                            .clamp(0.0, (w - 1) as f32) as usize;
                        let sy = (y as f32 + margin as f32 + ny * bend * factor + 0.5)
                            .clamp(0.0, (h - 1) as f32) as usize;
                        sy * w + sx
                    };
                    let green = sample(1.0);
                    let depth = if settings.depth {
                        let inner =
                            (1.0 - distance.max(0.0) / (corner * 0.5).max(0.5)).clamp(0.0, 1.0);
                        ny * inner * 18.0
                    } else {
                        0.0
                    };
                    let rim = (1.0 - distance.abs()).clamp(0.0, 1.0) * 45.0;
                    self.lens[y * ow + x] = Lens {
                        red: if settings.chromatic {
                            sample(1.18)
                        } else {
                            green
                        },
                        green,
                        blue: if settings.chromatic {
                            sample(0.82)
                        } else {
                            green
                        },
                        alpha: (coverage * 255.0) as u8,
                        shade: depth + rim,
                    };
                }
            }
        }
        // Geometry is stable while scrolling: only sample, tint and write.
        let opacity = settings.opacity;
        for (pixel, lens) in pixels.iter_mut().zip(&self.lens) {
            let rgb = [
                self.a[lens.red].red,
                self.a[lens.green].green,
                self.a[lens.blue].blue,
            ];
            let channel = |i: usize| {
                (rgb[i] as f32 * (1.0 - opacity) + settings.tint[i] as f32 * opacity + lens.shade)
                    .clamp(0.0, 255.0) as u8
            };
            *pixel = Rgba8Pixel::new(channel(0), channel(1), channel(2), lens.alpha);
        }
        image.clone()
    }
}

/// Sliding box filter: constant work per pixel regardless of radius.
fn blur(src: &[Pixel], dst: &mut [Pixel], w: usize, h: usize, radius: usize, vertical: bool) {
    let (lines, length, step) = if vertical { (w, h, w) } else { (h, w, 1) };
    let count = (radius * 2 + 1) as u32;
    // One reciprocal replaces four integer divisions per output pixel.
    let reciprocal = (1u64 << 32).div_ceil(count as u64);
    let average = |sum: u32| ((sum as u64 * reciprocal) >> 32) as u8;
    for line in 0..lines {
        let base = if vertical { line } else { line * w };
        let at = |i: usize| src[base + i * step];
        let mut sum = [0u32; 4];
        let add = |s: &mut [u32; 4], p: Pixel| {
            for (v, c) in s.iter_mut().zip([p.red, p.green, p.blue, p.alpha]) {
                *v += c as u32;
            }
        };
        for i in 0..=radius * 2 {
            add(&mut sum, at(i.saturating_sub(radius).min(length - 1)));
        }
        for i in 0..length {
            dst[base + i * step] = Pixel {
                red: average(sum[0]),
                green: average(sum[1]),
                blue: average(sum[2]),
                alpha: average(sum[3]),
            };
            let old = at(i.saturating_sub(radius));
            for (v, c) in sum
                .iter_mut()
                .zip([old.red, old.green, old.blue, old.alpha])
            {
                *v -= c as u32;
            }
            add(&mut sum, at((i + radius + 1).min(length - 1)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> Settings {
        Settings {
            blur: 6.0,
            refraction: 0.5,
            chromatic: true,
            depth: true,
            tint: [12, 12, 14],
            opacity: 0.4,
            scale: 1.0,
        }
    }
    #[test]
    fn blur_spreads_impulse_and_preserves_constant() {
        let mut src = vec![
            Pixel {
                red: 90,
                green: 60,
                blue: 30,
                alpha: 255
            };
            25
        ];
        let mut dst = src.clone();
        blur(&src, &mut dst, 5, 5, 1, false);
        assert!(dst
            .iter()
            .all(|p| [p.red, p.green, p.blue, p.alpha] == [90, 60, 30, 255]));
        src.fill(Pixel::default());
        src[12].red = 255;
        blur(&src, &mut dst, 5, 5, 1, false);
        assert_eq!([dst[11].red, dst[12].red, dst[13].red], [85; 3]);
    }
    #[test]
    fn lens_masks_corners_and_changes_edges() {
        let src: Vec<_> = (0..80 * 40)
            .map(|i| Pixel {
                red: (i % 80 * 3) as u8,
                green: (i / 80 * 6) as u8,
                blue: 90,
                alpha: 255,
            })
            .collect();
        let mut filter = Glass::default();
        let rect = Rect::new(0, 0, 80, 40);
        let mut plain = settings();
        plain.blur = 0.0;
        plain.refraction = 0.0;
        plain.chromatic = false;
        plain.depth = false;
        let a = filter.render(&src, 80, 40, rect, plain);
        let mut lens = plain;
        lens.refraction = 0.8;
        let refracted = filter.render(&src, 80, 40, rect, lens);
        assert_ne!(
            a.as_slice(),
            refracted.as_slice(),
            "refraction bends the gradient without blur"
        );
        let mut depth = plain;
        depth.depth = true;
        let shaded = filter.render(&src, 80, 40, rect, depth);
        assert_ne!(
            a.as_slice(),
            shaded.as_slice(),
            "depth works with refraction disabled"
        );
        let b = filter.render(&src, 80, 40, rect, settings());
        assert_eq!(b.as_slice()[0].a, 0);
        assert_eq!(b.as_slice()[20 * 10 + 20].a, 255);
        assert_ne!(a.as_slice(), b.as_slice());
    }
    #[test]
    fn reuses_storage_and_cached_image() {
        let mut src = vec![
            Pixel {
                red: 80,
                green: 60,
                blue: 40,
                alpha: 255
            };
            80 * 40
        ];
        let mut filter = Glass::default();
        let rect = Rect::new(0, 0, 80, 40);
        let first = filter.render(&src, 80, 40, rect, settings());
        let ptr = first.as_slice().as_ptr();
        let cached = filter.render(&src, 80, 40, rect, settings());
        assert_eq!(ptr, cached.as_slice().as_ptr());
        drop(first);
        drop(cached);
        let scratch = filter.a.as_ptr();
        src[20 * 80 + 40].red = 200;
        drop(filter.render(&src, 80, 40, rect, settings()));
        src[20 * 80 + 40].red = 210;
        let changed = filter.render(&src, 80, 40, rect, settings());
        assert_eq!(ptr, changed.as_slice().as_ptr(), "reuse alternating outputs");
        assert_eq!(scratch, filter.a.as_ptr());
    }

    #[test]
    #[ignore = "run in release mode with --nocapture to measure the glass pass"]
    fn benchmark_dock() {
        let mut src = vec![
            Pixel {
                red: 60,
                green: 90,
                blue: 120,
                alpha: 255
            };
            1080 * 300
        ];
        let mut filter = Glass::default();
        let rect = Rect::new(0, 50, 1080, 200);
        let mut s = settings();
        s.scale = 3.0;
        let cold = std::time::Instant::now();
        drop(filter.render(&src, 1080, 300, rect, s));
        eprintln!(
            "1080x200 initial glass setup: {:.3} ms",
            cold.elapsed().as_secs_f64() * 1000.0
        );
        let start = std::time::Instant::now();
        for i in 0..100 {
            src[80 * 1080 + 30].red = i;
            drop(filter.render(&src, 1080, 300, rect, s));
        }
        eprintln!(
            "1080x200 glass pass: {:.3} ms",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}
