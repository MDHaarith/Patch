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

pub fn sync_worried(configured: bool, offline: Option<std::time::Duration>) -> bool {
    configured && offline.is_some_and(|d| d > std::time::Duration::from_secs(3600))
}

/// Signed JNI codes keep the direction with each held/flight/bump pose.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Play {
    #[default]
    Idle,
    Held(i32),
    Flying(i32),
    Bump(i32),
    Landing,
    Dizzy,
}

impl Play {
    pub fn from_code(code: i32) -> Self {
        match code {
            -1 | 1 => Self::Held(code.signum()),
            -2 | 2 => Self::Flying(code.signum()),
            -3 | 3 => Self::Bump(code.signum()),
            4 => Self::Landing,
            5 => Self::Dizzy,
            _ => Self::Idle,
        }
    }

    pub fn recovery(self) -> Option<(u64, Self)> {
        match self {
            Self::Bump(dir) => Some((180, Self::Flying(dir))),
            Self::Landing => Some((360, Self::Idle)),
            Self::Dizzy => Some((900, Self::Idle)),
            _ => None,
        }
    }
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];
const OUTLINE: [u8; 4] = [0x2E, 0x18, 0x12, 255];

/// Body colours as (name, [body, highlight, shade]); index 0 is the default.
pub const SKINS: [(&str, [[u8; 4]; 3]); 8] = [
    ("Tangerine", [[0xFF, 0x6A, 0x1F, 255], [0xFF, 0xA0, 0x5C, 255], [0xD9, 0x48, 0x0A, 255]]),
    ("Cherry", [[0xFF, 0x2D, 0x55, 255], [0xFF, 0x7A, 0x93, 255], [0xC7, 0x10, 0x3A, 255]]),
    ("Bubblegum", [[0xFF, 0x4F, 0xB8, 255], [0xFF, 0x95, 0xD6, 255], [0xD1, 0x25, 0x8E, 255]]),
    ("Grape", [[0xAF, 0x52, 0xDE, 255], [0xD0, 0x8C, 0xF5, 255], [0x7E, 0x2B, 0xB0, 255]]),
    ("Electric", [[0x1E, 0x90, 0xFF, 255], [0x6C, 0xB8, 0xFF, 255], [0x0A, 0x62, 0xD1, 255]]),
    ("Aqua", [[0x00, 0xD1, 0xC1, 255], [0x5C, 0xF0, 0xE3, 255], [0x00, 0xA0, 0x90, 255]]),
    ("Lime", [[0x4C, 0xD9, 0x64, 255], [0x8B, 0xF0, 0x9A, 255], [0x23, 0xA8, 0x3F, 255]]),
    ("Sunshine", [[0xFF, 0xCC, 0x00, 255], [0xFF, 0xE4, 0x5C, 255], [0xE0, 0xA0, 0x00, 255]]),
];
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
/// shuffle; `blink` closes the eyes for a frame; `skin` indexes [`SKINS`].
pub fn frame(mood: Mood, tick: u32, blink: bool, skin: usize) -> Vec<[u8; 4]> {
    play_frame(mood, tick, blink, skin, Play::Idle)
}

pub fn play_frame(mood: Mood, tick: u32, blink: bool, skin: usize, play: Play) -> Vec<[u8; 4]> {
    let [body, light, shade] = SKINS[skin % SKINS.len()].1;
    let mut c = Canvas([CLEAR; W * H]);
    let up = tick.is_multiple_of(2) && mood != Mood::Sleepy;
    let top = match play {
        Play::Held(_) => 3,
        Play::Landing => 2,
        Play::Bump(_) => 5,
        _ => if up { 3 } else { 4 }
    };
    let insets = if matches!(play, Play::Bump(_)) { &INSETS[2..] } else { &INSETS[..] };
    let bottom = top + insets.len() as i32 - 1;
    let inside = |x: i32, y: i32| {
        (top..=bottom).contains(&y) && {
            let i = insets[(y - top) as usize];
            x >= BX0 + i && x <= BX1 - i
        }
    };

    // Arms sit behind the body: little nubs that wave when happy or worried.
    let wave = matches!(play, Play::Flying(_) | Play::Bump(_))
        || (play == Play::Idle && matches!(mood, Mood::Happy | Mood::Worried) && tick % 4 < 2);
    for (side, ax) in [(-1, BX0 - 1), (1, BX1 + 1)] {
        let lift = if wave && (side == 1 || mood == Mood::Worried) { 2 } else { 0 };
        let ay = top + 7 - lift;
        let ox = ax + side;
        for y in ay..=ay + 1 {
            c.set(ox, y, OUTLINE);
        }
        c.set(ax, ay - 1, OUTLINE);
        c.set(ax, ay + 2, OUTLINE);
        c.set(ax, ay, body);
        c.set(ax, ay + 1, shade);
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
                shade
            } else if !inside(x, y - 2) && x < 11 || !inside(x - 2, y) && y < top + 6 {
                light
            } else {
                body
            };
            c.set(x, y, fill);
        }
    }
    // A shine on the forehead.
    c.set(6, top + 2, GLINT);
    c.set(7, top + 2, light);

    // Sprout of thread on top; it sways with the bob.
    let sway = if up { 0 } else { 1 };
    let sprout_top = top.max(3);
    c.set(10, sprout_top - 1, OUTLINE);
    c.set(10 + sway, sprout_top - 2, OUTLINE);
    c.set(11 + sway, sprout_top - 3, body);
    c.set(12 + sway, sprout_top - 3, light);
    c.set(11 + sway, sprout_top - 2, shade);

    // Feet shuffle.
    let (lf, rf) = if tick % 4 < 2 { (0, 1) } else { (1, 0) };
    for (x, lift) in [(6, lf), (12, rf)] {
        let lifted = mood != Mood::Sleepy && lift == 1;
        let fy = if matches!(play, Play::Held(_)) {
            bottom + 2
        } else if lifted {
            bottom
        } else {
            bottom + 1
        };
        c.set(x, fy, OUTLINE);
        c.set(x + 1, fy, OUTLINE);
        if matches!(play, Play::Held(_)) {
            c.set(x, bottom + 1, OUTLINE);
            c.set(x + 1, bottom + 1, OUTLINE);
        }
        if !lifted {
            c.set(x, bottom, OUTLINE);
            c.set(x + 1, bottom, OUTLINE);
        }
    }

    // Face: big eyes with a glint, rosy cheeks.
    let ey = top + 4;
    let look = if play == Play::Idle && tick % 32 == 24 { 1 } else { 0 };
    let eyes = [6 + look, 12 + look];
    let cheeks = |c: &mut Canvas| {
        for x in [5, 14] {
            c.set(x, ey + 3, CHEEK);
            c.set(x + 1, ey + 3, CHEEK);
        }
    };
    match (play, mood) {
        (Play::Idle, Mood::Happy) => {
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
        (Play::Idle, Mood::Sleepy) => {
            for ex in eyes {
                c.set(ex - 1, ey + 2, EYE);
                c.set(ex, ey + 2, EYE);
                c.set(ex + 1, ey + 2, EYE);
            }
            c.set(9, ey + 4, EYE);
            c.set(10, ey + 4, EYE);
            if tick % 32 == 26 {
                for y in ey + 3..=ey + 5 {
                    c.set(9, y, EYE);
                    c.set(10, y, EYE);
                }
            }
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
        (Play::Idle, Mood::Busy | Mood::Worried) => {
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
        _ => {
            for ex in eyes {
                match play {
                    Play::Dizzy => {
                        for (dx, dy) in [(-1, 0), (1, 0), (0, 1), (-1, 2), (1, 2)] {
                            c.set(ex + dx, ey + dy, EYE);
                        }
                    }
                    Play::Flying(_) | Play::Bump(_) => {
                        for (dx, dy) in [(-1, 1), (0, 0), (1, 1)] {
                            c.set(ex + dx, ey + dy, EYE);
                        }
                    }
                    _ => {
                        for dy in 0..3 {
                            c.set(ex, ey + dy, EYE);
                            c.set(ex + 1, ey + dy, EYE);
                        }
                        c.set(ex, ey, GLINT);
                    }
                }
            }
            for x in 9..=10 {
                c.set(x, ey + 3, EYE);
                c.set(x, ey + 4, if matches!(play, Play::Held(_)) { EYE } else { CHEEK });
            }
            cheeks(&mut c);
        }
    }
    if matches!(play, Play::Held(-1) | Play::Flying(-1) | Play::Bump(-1)) {
        for row in c.0.as_chunks_mut::<W>().0 {
            row.reverse();
        }
    }
    c.0.to_vec()
}

pub fn image(mood: Mood, tick: u32, blink: bool, skin: usize) -> Image {
    play_image(mood, tick, blink, skin, Play::Idle)
}

pub fn play_image(mood: Mood, tick: u32, blink: bool, skin: usize, play: Play) -> Image {
    let px = play_frame(mood, tick, blink, skin, play);
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
    fn sync_trouble_needs_a_continuous_hour() {
        use std::time::Duration;
        assert!(!sync_worried(false, Some(Duration::from_secs(7200))));
        assert!(!sync_worried(true, None));
        assert!(!sync_worried(true, Some(Duration::from_secs(3600))));
        assert!(sync_worried(true, Some(Duration::from_secs(3601))));
    }

    #[test]
    fn frames_differ_by_mood_and_tick() {
        let busy0 = frame(Mood::Busy, 0, false, 0);
        assert_ne!(busy0, frame(Mood::Busy, 1, false, 0), "it bobs");
        assert_ne!(busy0, frame(Mood::Busy, 0, true, 0), "it blinks");
        assert_ne!(busy0, frame(Mood::Happy, 0, false, 0));
        assert_ne!(busy0, frame(Mood::Worried, 0, false, 0));
        assert_ne!(busy0, frame(Mood::Sleepy, 0, false, 0));
        assert_ne!(busy0, frame(Mood::Busy, 0, false, 1), "skins recolour");
        assert_eq!(frame(Mood::Busy, 0, false, SKINS.len()), busy0, "out-of-range skin wraps");
        assert!(busy0.iter().filter(|p| p[3] > 0).count() > 120, "a solid little body");
    }

    #[test]
    fn play_frames_are_crisp_distinct_and_directional() {
        for skin in 0..SKINS.len() {
            for mood in [Mood::Happy, Mood::Busy, Mood::Worried, Mood::Sleepy] {
                let idle = frame(mood, 0, false, skin);
                let poses: Vec<_> = [1, 2, 3, 4, 5]
                    .into_iter()
                    .map(|code| play_frame(mood, 0, false, skin, Play::from_code(code)))
                    .collect();
                for (i, pose) in poses.iter().enumerate() {
                    assert_eq!(pose.len(), W * H);
                    assert!(pose.iter().all(|p| p[3] == 0 || p[3] == 255));
                    assert_ne!(pose, &idle);
                    assert!(!poses[..i].contains(pose), "each reaction has its own pose");
                }
                for code in 1..=3 {
                    let mut right = play_frame(mood, 0, false, skin, Play::from_code(code));
                    for row in right.as_chunks_mut::<W>().0 {
                        row.reverse();
                    }
                    assert_eq!(
                        right,
                        play_frame(mood, 0, false, skin, Play::from_code(-code))
                    );
                }
            }
        }
        assert_ne!(
            frame(Mood::Busy, 0, false, 0),
            frame(Mood::Busy, 24, false, 0),
            "looks around"
        );
        assert_ne!(
            frame(Mood::Sleepy, 2, false, 0),
            frame(Mood::Sleepy, 26, false, 0),
            "yawns"
        );
    }

    #[test]
    fn play_recovery_preserves_flight_and_rejects_unknown_codes() {
        for code in [i32::MIN, -5, -4, 0, 6, i32::MAX] {
            assert_eq!(Play::from_code(code), Play::Idle);
        }
        assert_eq!(Play::Bump(-1).recovery(), Some((180, Play::Flying(-1))));
        assert_eq!(Play::Landing.recovery(), Some((360, Play::Idle)));
        assert_eq!(Play::Dizzy.recovery(), Some((900, Play::Idle)));
        for play in [Play::Idle, Play::Held(1), Play::Flying(-1)] {
            assert_eq!(play.recovery(), None);
        }
    }
}
