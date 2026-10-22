//! Patch the pet: a chunky little critter drawn as pixel art, in the spirit
//! of the terminal pets. Frames are generated here (no image decoding) and
//! shown with nearest-neighbour scaling, so it stays crisp at any size.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub const W: usize = 20;
pub const H: usize = 18;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Happy,
    Busy,
    Worried,
    Sleepy,
}

impl Mood {
    pub fn from_index(i: i32) -> Mood {
        match i {
            0 => Mood::Happy,
            2 => Mood::Worried,
            3 => Mood::Sleepy,
            _ => Mood::Busy,
        }
    }
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];
const OUTLINE: [u8; 4] = [0x2E, 0x18, 0x12, 255];
const BODY: [u8; 4] = [0xF0, 0x8A, 0x5D, 255];
const LIGHT: [u8; 4] = [0xFF, 0xB8, 0x8F, 255];
const SHADE: [u8; 4] = [0xC8, 0x5F, 0x3A, 255];
const EYE: [u8; 4] = [0x1A, 0x10, 0x0E, 255];
const GLINT: [u8; 4] = [0xFF, 0xFF, 0xFF, 255];
const CHEEK: [u8; 4] = [0xFF, 0x6F, 0x7D, 255];
const DROP: [u8; 4] = [0x64, 0xD2, 0xFF, 255];
const ZZZ: [u8; 4] = [0xC8, 0xC8, 0xD2, 255];
const SPARK: [u8; 4] = [0xFF, 0xD6, 0x0A, 255];

struct Canvas([[u8; 4]; W * H]);

impl Canvas {
    fn set(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            self.0[y as usize * W + x as usize] = c;
        }
    }
}

/// Row insets of the body silhouette, top to bottom: a soft mochi shape,
/// rounder on top and flatter at the base.
const INSETS: [i32; 11] = [4, 2, 1, 1, 0, 0, 0, 0, 0, 0, 1];
const BX0: i32 = 3;
const BX1: i32 = 16;

/// One animation frame. `tick` drives the bob, the arm wave and the foot
/// shuffle; `blink` closes the eyes for a frame.
pub fn frame(mood: Mood, tick: u32, blink: bool) -> Vec<[u8; 4]> {
    let mut c = Canvas([CLEAR; W * H]);
    let up = tick.is_multiple_of(2) && mood != Mood::Sleepy;
    let top = if up { 3 } else { 4 };
    let bottom = top + INSETS.len() as i32 - 1;
    let inside = |x: i32, y: i32| {
        (top..=bottom).contains(&y) && {
            let i = INSETS[(y - top) as usize];
            x >= BX0 + i && x <= BX1 - i
        }
    };

    // Arms sit behind the body: little nubs that wave when happy or worried.
    let wave = matches!(mood, Mood::Happy | Mood::Worried) && tick % 4 < 2;
    for (side, ax) in [(-1, BX0 - 1), (1, BX1 + 1)] {
        let lift = if wave && (side == 1 || mood == Mood::Worried) { 2 } else { 0 };
        let ay = top + 7 - lift;
        let ox = ax + side;
        for y in ay..=ay + 1 {
            c.set(ox, y, OUTLINE);
        }
        c.set(ax, ay - 1, OUTLINE);
        c.set(ax, ay + 2, OUTLINE);
        c.set(ax, ay, BODY);
        c.set(ax, ay + 1, SHADE);
    }

    // Body: outline where the silhouette meets the outside, then shading.
    for y in top..=bottom {
        for x in BX0..=BX1 {
            if !inside(x, y) {
                continue;
            }
            let edge = !inside(x - 1, y) || !inside(x + 1, y) || !inside(x, y - 1) || !inside(x, y + 1);
            let fill = if edge {
                OUTLINE
            } else if !inside(x, y + 2) || !inside(x + 2, y) {
                SHADE
            } else if !inside(x, y - 2) && x < 11 || !inside(x - 2, y) && y < top + 6 {
                LIGHT
            } else {
                BODY
            };
            c.set(x, y, fill);
        }
    }
    // A shine on the forehead.
    c.set(6, top + 2, GLINT);
    c.set(7, top + 2, LIGHT);

    // Sprout of thread on top; it sways with the bob.
    let sway = if up { 0 } else { 1 };
    c.set(10, top - 1, OUTLINE);
    c.set(10 + sway, top - 2, OUTLINE);
    c.set(11 + sway, top - 3, BODY);
    c.set(12 + sway, top - 3, LIGHT);
    c.set(11 + sway, top - 2, SHADE);

    // Feet shuffle.
    let (lf, rf) = if tick % 4 < 2 { (0, 1) } else { (1, 0) };
    for (x, lift) in [(6, lf), (12, rf)] {
        let lifted = mood != Mood::Sleepy && lift == 1;
        let fy = if lifted { bottom } else { bottom + 1 };
        c.set(x, fy, OUTLINE);
        c.set(x + 1, fy, OUTLINE);
        if !lifted {
            c.set(x, bottom, OUTLINE);
            c.set(x + 1, bottom, OUTLINE);
        }
    }

    // Face: big eyes with a glint, rosy cheeks.
    let ey = top + 4;
    let eyes = [6, 12];
    let cheeks = |c: &mut Canvas| {
        for x in [5, 14] {
            c.set(x, ey + 3, CHEEK);
            c.set(x + 1, ey + 3, CHEEK);
        }
    };
    match mood {
        Mood::Happy => {
            for ex in eyes {
                c.set(ex - 1, ey + 2, EYE);
                c.set(ex, ey + 1, EYE);
                c.set(ex + 1, ey + 1, EYE);
                c.set(ex + 2, ey + 2, EYE);
            }
            for x in 9..=10 {
                c.set(x, ey + 3, EYE);
                c.set(x, ey + 4, CHEEK);
            }
            c.set(8, ey + 3, EYE);
            c.set(11, ey + 3, EYE);
            cheeks(&mut c);
            if tick.is_multiple_of(4) {
                for (x, y) in [(18, 1), (17, 2), (19, 2), (18, 3)] {
                    c.set(x, y, SPARK);
                }
            }
        }
        Mood::Sleepy => {
            for ex in eyes {
                c.set(ex - 1, ey + 2, EYE);
                c.set(ex, ey + 2, EYE);
                c.set(ex + 1, ey + 2, EYE);
            }
            c.set(9, ey + 4, EYE);
            c.set(10, ey + 4, EYE);
            cheeks(&mut c);
            let zy = if tick % 4 < 2 { 0 } else { 1 };
            for x in 15..=19 {
                c.set(x, zy, ZZZ);
                c.set(x, zy + 4, ZZZ);
            }
            for d in 1..=3 {
                c.set(19 - d, zy + d, ZZZ);
            }
        }
        Mood::Busy | Mood::Worried => {
            for ex in eyes {
                if blink {
                    c.set(ex, ey + 2, EYE);
                    c.set(ex + 1, ey + 2, EYE);
                } else {
                    for dy in 0..3 {
                        c.set(ex, ey + dy, EYE);
                        c.set(ex + 1, ey + dy, EYE);
                    }
                    c.set(ex, ey, GLINT);
                }
            }
            if mood == Mood::Worried {
                // Tilted brows, a wobbly mouth and a sweat drop.
                c.set(5, ey - 1, EYE);
                c.set(6, ey - 2, EYE);
                c.set(14, ey - 1, EYE);
                c.set(13, ey - 2, EYE);
                for (x, y) in [(8, ey + 4), (9, ey + 3), (10, ey + 4), (11, ey + 3)] {
                    c.set(x, y, EYE);
                }
                let d = (tick % 2) as i32;
                c.set(16, top + 1 + d, DROP);
                c.set(16, top + 2 + d, DROP);
                c.set(17, top + 2 + d, DROP);
            } else {
                c.set(9, ey + 3, EYE);
                c.set(10, ey + 3, EYE);
            }
            cheeks(&mut c);
        }
    }
    c.0.to_vec()
}

pub fn image(mood: Mood, tick: u32, blink: bool) -> Image {
    let px = frame(mood, tick, blink);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(W as u32, H as u32);
    for (d, s) in buf.make_mut_slice().iter_mut().zip(px) {
        *d = Rgba8Pixel { r: s[0], g: s[1], b: s[2], a: s[3] };
    }
    Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_differ_by_mood_and_tick() {
        let busy0 = frame(Mood::Busy, 0, false);
        assert_ne!(busy0, frame(Mood::Busy, 1, false), "it bobs");
        assert_ne!(busy0, frame(Mood::Busy, 0, true), "it blinks");
        assert_ne!(busy0, frame(Mood::Happy, 0, false));
        assert_ne!(busy0, frame(Mood::Worried, 0, false));
        assert_ne!(busy0, frame(Mood::Sleepy, 0, false));
        assert!(busy0.iter().filter(|p| p[3] > 0).count() > 120, "a solid little body");
    }
}
