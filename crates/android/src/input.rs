//! Translation of Android touch and key events into Slint window events.

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{LogicalPosition, SharedString};

/// `MotionEvent` actions, as numbered by `PatchNative.java`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchAction {
    Down,
    Move,
    Up,
    Cancel,
    LongPress,
}

impl TouchAction {
    pub fn from_java(action: i32) -> Option<TouchAction> {
        Some(match action {
            0 => TouchAction::Down,
            1 => TouchAction::Move,
            2 => TouchAction::Up,
            3 => TouchAction::Cancel,
            4 => TouchAction::LongPress,
            _ => return None,
        })
    }
}

/// A finger is a mouse that only exists while touching: hover must not
/// linger after it lifts, or the last-touched control would stay highlighted.
pub fn touch_events(action: TouchAction, position: LogicalPosition) -> Vec<WindowEvent> {
    let button = PointerEventButton::Left;
    match action {
        TouchAction::Down => vec![
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed { position, button },
        ],
        TouchAction::Move => vec![WindowEvent::PointerMoved { position }],
        TouchAction::Up => vec![
            WindowEvent::PointerReleased { position, button },
            WindowEvent::PointerExited,
        ],
        TouchAction::Cancel => vec![WindowEvent::PointerExited],
        TouchAction::LongPress => Vec::new(),
    }
}

/// The Slint key text for an Android `KeyEvent`: special keys by key code,
/// everything else by the character it produces (`getUnicodeChar`).
pub fn key_text(key_code: i32, unicode: u32) -> Option<SharedString> {
    let key = match key_code {
        19 => Key::UpArrow,
        20 => Key::DownArrow,
        21 => Key::LeftArrow,
        22 => Key::RightArrow,
        61 => Key::Tab,
        66 | 160 => Key::Return,
        67 => Key::Backspace,
        112 => Key::Delete,
        111 => Key::Escape,
        92 => Key::PageUp,
        93 => Key::PageDown,
        122 => Key::Home,
        123 => Key::End,
        59 | 60 => Key::Shift,
        57 | 58 => Key::Alt,
        113 | 114 => Key::Control,
        117 | 118 => Key::Meta,
        _ => {
            return char::from_u32(unicode)
                .filter(|c| *c != '\0' && !c.is_control())
                .map(|c| c.to_string().into());
        }
    };
    Some(key.into())
}

/// Logical position of a point given in surface pixels.
pub fn logical(x: f32, y: f32, scale: f32) -> LogicalPosition {
    LogicalPosition::new(x / scale, y / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(key_text(67, 0), Some(Key::Backspace.into()));
        assert_eq!(key_text(66, 10), Some(Key::Return.into()));
        assert_eq!(key_text(29, 'a' as u32), Some("a".into()));
        assert_eq!(key_text(0, 0), None);
        assert_eq!(key_text(0, 0x1b), None, "control characters are not text");
    }

    #[test]
    fn taps_clear_hover() {
        let p = logical(262.5, 525.0, 2.625);
        assert_eq!((p.x, p.y), (100.0, 200.0));
        let up = touch_events(TouchAction::Up, p);
        assert!(matches!(up.last(), Some(WindowEvent::PointerExited)));
        assert_eq!(touch_events(TouchAction::Down, p).len(), 2);
    }
}
