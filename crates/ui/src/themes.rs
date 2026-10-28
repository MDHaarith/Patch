//! The theme catalogue. Each theme is a handful of base colours; the rest of
//! the tokens are derived so every theme stays coherent.

use crate::models::color;
use crate::{Theme, ThemeCard};
use slint::Color;

#[derive(Clone, Copy)]
enum Kind {
    Matte,
    Light,
}

pub struct Spec {
    pub name: &'static str,
    kind: Kind,
    /// Backdrop gradient, top to bottom.
    top: &'static str,
    bottom: &'static str,
    /// Surface colour for panels.
    surface: &'static str,
    accent: &'static str,
}

pub const THEMES: [Spec; 2] = [
    Spec {
        name: "Matte Black",
        kind: Kind::Matte,
        top: "#0B0B0C",
        bottom: "#060607",
        surface: "#121214",
        accent: "#0A84FF",
    },
    Spec {
        name: "Light",
        kind: Kind::Light,
        top: "#F5F5F7",
        bottom: "#ECECF0",
        surface: "#FFFFFF",
        accent: "#007AFF",
    },
];

fn white(a: f32) -> Color {
    Color::from_argb_f32(a, 1.0, 1.0, 1.0)
}

fn black(a: f32) -> Color {
    Color::from_argb_f32(a, 0.0, 0.0, 0.0)
}

/// Mixes `b` into `a` by `t`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::from_argb_u8(
        l(a.alpha(), b.alpha()),
        l(a.red(), b.red()),
        l(a.green(), b.green()),
        l(a.blue(), b.blue()),
    )
}

pub fn spec(index: i32) -> &'static Spec {
    THEMES.get(index.max(0) as usize).unwrap_or(&THEMES[0])
}

pub fn apply(t: &Theme, index: i32) {
    let s = spec(index);
    let (top, bottom, surface, accent) = (
        color(s.top),
        color(s.bottom),
        color(s.surface),
        color(s.accent),
    );
    t.set_pet(match s.kind {
        Kind::Light => color("#1D1D20"),
        Kind::Matte => mix(surface, white(1.0), 0.08),
    });
    t.set_backdrop_top(top);
    t.set_backdrop_bottom(bottom);
    t.set_accent(accent);
    match s.kind {
        Kind::Light => {
            t.set_light(true);
            t.set_solid(top);
            t.set_panel(surface.with_alpha(0.85));
            t.set_panel_strong(surface);
            t.set_content(surface.with_alpha(0.8));
            t.set_float(surface.with_alpha(0.96));
            t.set_modal(mix(top, surface, 0.5).with_alpha(0.97));
            t.set_dock(surface.with_alpha(0.92));
            t.set_card(surface);
            t.set_picker(surface);
            t.set_bubble_top(surface);
            t.set_bubble_bottom(mix(surface, bottom, 0.6));
            t.set_well(black(0.04));
            t.set_hairline(black(0.1));
            t.set_edge(black(0.08));
            t.set_sheen_color(white(1.0));
            t.set_sheen_reach(1.0);
            t.set_shadow(black(0.12));
            t.set_text(color("#1D1D1F"));
            t.set_text_2(color("#6E6E73"));
            t.set_text_3(color("#AEAEB2"));
            t.set_today(color("#FF3B30"));
            t.set_danger(color("#FF3B30"));
            t.set_success(color("#34C759"));
            t.set_warning(color("#FF9500"));
            t.set_hover(black(0.05));
            t.set_press(black(0.1));
            t.set_range(mix(bottom, black(1.0), 0.06));
            t.set_chip(mix(bottom, black(1.0), 0.04));
            t.set_weekend(black(0.02));
            t.set_toast(color("#1D1D1F").with_alpha(0.9));
            t.set_toast_text(white(1.0));
        }
        Kind::Matte => {
            t.set_light(false);
            t.set_solid(mix(top, bottom, 0.55));
            let lift = |a: f32| mix(surface, white(1.0), a);
            t.set_panel(surface);
            t.set_panel_strong(lift(0.04));
            t.set_content(mix(surface, bottom, 0.5));
            t.set_float(lift(0.02).with_alpha(0.95));
            t.set_modal(mix(surface, bottom, 0.3).with_alpha(0.97));
            t.set_dock(lift(0.03).with_alpha(0.95));
            t.set_card(lift(0.05));
            t.set_picker(lift(0.06));
            t.set_bubble_top(lift(0.12));
            t.set_bubble_bottom(mix(surface, bottom, 0.6));
            t.set_well(white(0.04));
            t.set_hairline(white(0.07));
            t.set_edge(white(0.12));
            t.set_sheen_color(white(0.07));
            t.set_sheen_reach(0.45);
            t.set_text_2(color("#9A9AA1"));
            t.set_text_3(color("#5A5A60"));
            t.set_range(lift(0.16));
            t.set_chip(lift(0.1));
            t.set_shadow(black(0.6));
            t.set_text(color("#F5F5F7"));
            t.set_today(color("#FF453A"));
            t.set_danger(color("#FF453A"));
            t.set_success(color("#30D158"));
            t.set_warning(color("#FF9F0A"));
            t.set_hover(white(0.06));
            t.set_press(white(0.1));
            t.set_weekend(white(0.016));
            t.set_toast(color("#F5F5F7").with_alpha(0.92));
            t.set_toast_text(color("#111113"));
        }
    }
}

/// Preview swatches for the theme picker.
pub fn cards() -> Vec<ThemeCard> {
    THEMES
        .iter()
        .map(|s| {
            let light = matches!(s.kind, Kind::Light);
            let surface = color(s.surface);
            ThemeCard {
                name: s.name.into(),
                top: color(s.top),
                bottom: color(s.bottom),
                mid: mix(color(s.top), color(s.bottom), 0.5),
                card: if light {
                    surface
                } else {
                    mix(surface, white(1.0), 0.06)
                },
                ink: if light {
                    color("#1D1D1F")
                } else {
                    color("#F5F5F7")
                },
                band: if light {
                    mix(color(s.bottom), black(1.0), 0.08)
                } else {
                    mix(surface, white(1.0), 0.18)
                },
                accent: color(s.accent),
                edge: if light { black(0.08) } else { white(0.12) },
            }
        })
        .collect()
}
