//! Soft keyboard (IME) support.
//!
//! Slint's `TextInput` announces focus, text and cursor changes through
//! `WindowAdapterInternal::input_method_request`, and accepts composition
//! ("pre-edit") through internal key events; neither is reachable through the
//! public platform API, hence the `i-slint-core` dependency.
//!
//! The Java `InputConnection` keeps a mirror `Editable` so the IME's queries
//! (`getTextBeforeCursor` and friends) are answered instantly, and forwards
//! every edit here tagged with a sequence number. We apply the edit to the
//! focused `TextInput` and echo the resulting state back with the number of
//! the last edit applied; Java resynchronises its mirror (and restarts the
//! IME) whenever Slint disagrees, e.g. digits-only fields or a field the app
//! cleared after Enter.

use i_slint_core::input::{InternalKeyEvent, KeyEventType};
use i_slint_core::items::{CapitalizationMode, InputType};
use i_slint_core::window::{InputMethodProperties, InputMethodRequest};
use slint::platform::{Key, WindowEvent};
use slint::SharedString;

/// An edit from the Java `InputConnection`. Positions are UTF-16 offsets into
/// the text as the IME sees it: the committed text with the composition
/// inlined at the cursor. The numeric codes are shared with `PatchNative.java`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeOp {
    Commit(String),
    Compose(String),
    FinishComposing,
    DeleteSurrounding { before: u32, after: u32 },
    SetSelection { start: u32, end: u32 },
    SetComposingRegion { start: u32, end: u32 },
    EditorAction,
}

impl ImeOp {
    pub fn from_java(op: i32, text: String, a: i32, b: i32) -> Option<ImeOp> {
        let (a, b) = (a.max(0) as u32, b.max(0) as u32);
        Some(match op {
            0 => ImeOp::Commit(text),
            1 => ImeOp::Compose(text),
            2 => ImeOp::FinishComposing,
            3 => ImeOp::DeleteSurrounding {
                before: a,
                after: b,
            },
            4 => ImeOp::SetSelection {
                start: a.min(b),
                end: a.max(b),
            },
            5 => ImeOp::SetComposingRegion {
                start: a.min(b),
                end: a.max(b),
            },
            6 => ImeOp::EditorAction,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImeMode {
    /// No editable field has focus: hide the keyboard.
    Hide = 0,
    /// Same field, new text/selection.
    Update = 1,
    /// A field (re)gained focus or was tapped: restart input and show.
    Show = 2,
}

/// The editor state the Java side presents to the IME.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImeSnapshot {
    pub text: String,
    pub sel_start: i32,
    pub sel_end: i32,
    /// `-1` when nothing is being composed.
    pub comp_start: i32,
    pub comp_end: i32,
    /// `android.text.InputType` bits.
    pub input_type: i32,
    /// `EditorInfo.IME_ACTION_*`.
    pub action: i32,
}

/// IME bookkeeping for one window. Lives on the UI thread.
#[derive(Default)]
pub struct ImeState {
    /// Latest state of the focused editable `TextInput`; `None` without one.
    props: Option<InputMethodProperties>,
    /// The composition shown at the cursor. Slint only reports it with the
    /// next cursor/text change, so the edits we send keep it current.
    preedit: String,
    pending: Option<ImeMode>,
    /// Sequence number of the last Java edit applied.
    seq: i32,
}

impl ImeState {
    /// Records a request from Slint. Runs inside event dispatch or rendering,
    /// so it must not call back into Slint.
    pub fn request(&mut self, req: InputMethodRequest) {
        match req {
            InputMethodRequest::Enable(p) => {
                self.preedit = p.preedit_text.to_string();
                self.props = Some(p);
                self.pending = Some(ImeMode::Show);
            }
            InputMethodRequest::Update(p) => {
                let was_active = self.props.is_some();
                self.preedit = p.preedit_text.to_string();
                self.props = Some(p);
                let mode = if was_active {
                    ImeMode::Update
                } else {
                    ImeMode::Show
                };
                self.pending = Some(self.pending.map_or(mode, |m| {
                    if m == ImeMode::Hide {
                        mode
                    } else {
                        m.max(mode)
                    }
                }));
            }
            InputMethodRequest::Disable => {
                self.props = None;
                self.preedit.clear();
                self.pending = Some(ImeMode::Hide);
            }
            _ => {}
        }
    }

    pub fn is_active(&self) -> bool {
        self.props.is_some()
    }

    pub fn has_preedit(&self) -> bool {
        !self.preedit.is_empty()
    }

    /// Something to tell Java, if anything changed since the last call.
    pub fn take_pending(&mut self) -> Option<(ImeMode, ImeSnapshot, i32)> {
        let mode = self.pending.take()?;
        let snapshot = if mode == ImeMode::Hide {
            ImeSnapshot::default()
        } else {
            self.snapshot()?
        };
        Some((mode, snapshot, self.seq))
    }

    fn snapshot(&self) -> Option<ImeSnapshot> {
        let p = self.props.as_ref()?;
        let text = p.text.as_str();
        let cursor = floor_boundary(text, p.cursor_position);
        let anchor = floor_boundary(text, p.anchor_position.unwrap_or(cursor));
        let pre = self.preedit.as_str();
        let pre16 = utf16_len(pre) as i32;
        // The IME sees the composition inlined at the cursor, with the cursor
        // after it.
        let at = |b: usize| -> i32 {
            let p16 = utf16_len(&text[..b]) as i32;
            if b < cursor {
                p16
            } else {
                p16 + pre16
            }
        };
        let (cur16, anc16) = (
            at(cursor),
            if anchor == cursor {
                at(cursor)
            } else {
                at(anchor)
            },
        );
        let c16 = utf16_len(&text[..cursor]) as i32;
        let (input_type, action) = android_input_type(p);
        Some(ImeSnapshot {
            text: format!("{}{}{}", &text[..cursor], pre, &text[cursor..]),
            sel_start: cur16.min(anc16),
            sel_end: cur16.max(anc16),
            comp_start: if pre.is_empty() { -1 } else { c16 },
            comp_end: if pre.is_empty() { -1 } else { c16 + pre16 },
            input_type,
            action,
        })
    }

    /// Have Java restart the IME, e.g. after we settled its composition.
    pub fn restart(&mut self) {
        if self.props.is_some() {
            self.pending = Some(ImeMode::Show);
        }
    }

    /// A key event from Java was applied; echo so Java can match it up.
    pub fn applied(&mut self, seq: i32) {
        self.seq = self.seq.max(seq);
        if self.props.is_some() {
            self.pending = Some(
                self.pending
                    .map_or(ImeMode::Update, |m| m.max(ImeMode::Update)),
            );
        }
    }

    /// Commits a pending composition, e.g. before the user taps elsewhere.
    pub fn finish_composing(&mut self) -> Vec<WindowEvent> {
        self.translate(ImeOp::FinishComposing, self.seq)
    }

    /// Translates a Java edit into the events that perform it on the focused
    /// `TextInput`.
    pub fn translate(&mut self, op: ImeOp, seq: i32) -> Vec<WindowEvent> {
        self.seq = self.seq.max(seq);
        // Always echo, so Java learns its edit was applied (or ignored).
        self.pending = Some(
            self.pending
                .map_or(ImeMode::Update, |m| m.max(ImeMode::Update)),
        );
        let Some(p) = self.props.as_ref() else {
            self.pending = Some(ImeMode::Hide);
            return Vec::new();
        };
        let text = p.text.to_string();
        let cursor = floor_boundary(&text, p.cursor_position);
        let anchor = floor_boundary(&text, p.anchor_position.unwrap_or(cursor));

        let mut out = Vec::new();
        match op {
            ImeOp::Commit(t) => {
                self.preedit.clear();
                out.push(composition(
                    KeyEventType::CommitComposition,
                    &t,
                    "",
                    None,
                    None,
                ));
            }
            ImeOp::Compose(t) => {
                // Without a composition, composing replaces the selection.
                let range = (self.preedit.is_empty() && anchor != cursor)
                    .then(|| rel(anchor.min(cursor), cursor)..rel(anchor.max(cursor), cursor));
                self.preedit = t;
                out.push(composition(
                    KeyEventType::UpdateComposition,
                    "",
                    &self.preedit,
                    range,
                    None,
                ));
            }
            ImeOp::FinishComposing => {
                if !self.preedit.is_empty() {
                    let t = std::mem::take(&mut self.preedit);
                    out.push(composition(
                        KeyEventType::CommitComposition,
                        &t,
                        "",
                        None,
                        None,
                    ));
                }
            }
            ImeOp::DeleteSurrounding { before, after } => {
                // Android deletes around the composition, never inside it;
                // Slint keeps the composition out of `text`, so the cursor
                // already sits on its edge.
                let start = back_utf16(&text, cursor, before);
                let end = forward_utf16(&text, cursor, after);
                if start < end {
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        &self.preedit,
                        Some(rel(start, cursor)..rel(end, cursor)),
                        None,
                    ));
                }
            }
            ImeOp::SetSelection { start, end } => {
                let (full, _) = self.commit_into(&text, cursor, &mut out);
                let (s, e) = (byte_at_utf16(&full, start), byte_at_utf16(&full, end));
                out.push(composition(
                    KeyEventType::UpdateComposition,
                    "",
                    "",
                    None,
                    Some((e as i32, s as i32)),
                ));
            }
            ImeOp::SetComposingRegion { start, end } => {
                let (full, cur) = self.commit_into(&text, cursor, &mut out);
                let (s, e) = (byte_at_utf16(&full, start), byte_at_utf16(&full, end));
                if s < e {
                    self.preedit = full[s..e].to_string();
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        &self.preedit,
                        Some(rel(s, cur)..rel(e, cur)),
                        None,
                    ));
                }
            }
            ImeOp::EditorAction => {
                self.commit_into(&text, cursor, &mut out);
                let enter: SharedString = Key::Return.into();
                out.push(WindowEvent::KeyPressed {
                    text: enter.clone(),
                });
                out.push(WindowEvent::KeyReleased { text: enter });
            }
        }
        out
    }

    /// Commits the composition (if any) and returns the resulting text and
    /// cursor byte offset, as Slint will have them once `out` is dispatched.
    fn commit_into(
        &mut self,
        text: &str,
        cursor: usize,
        out: &mut Vec<WindowEvent>,
    ) -> (String, usize) {
        if self.preedit.is_empty() {
            return (text.to_string(), cursor);
        }
        let pre = std::mem::take(&mut self.preedit);
        out.push(composition(
            KeyEventType::CommitComposition,
            &pre,
            "",
            None,
            None,
        ));
        (
            format!("{}{}{}", &text[..cursor], pre, &text[cursor..]),
            cursor + pre.len(),
        )
    }
}

fn composition(
    kind: KeyEventType,
    commit: &str,
    preedit: &str,
    replacement_range: Option<std::ops::Range<i32>>,
    cursor_and_anchor: Option<(i32, i32)>,
) -> WindowEvent {
    let mut ev = InternalKeyEvent {
        event_type: kind,
        preedit_text: preedit.into(),
        replacement_range,
        cursor_position: cursor_and_anchor.map(|(cursor, _)| cursor),
        anchor_position: cursor_and_anchor.map(|(_, anchor)| anchor),
        ..Default::default()
    };
    ev.key_event.text = commit.into();
    WindowEvent::internal(ev)
}

fn rel(byte: usize, cursor: usize) -> i32 {
    byte as i32 - cursor as i32
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Byte offset of the UTF-16 offset `pos` in `s` (clamped; a position inside
/// a surrogate pair rounds down to the character's start).
pub fn byte_at_utf16(s: &str, pos: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in s.char_indices() {
        if units + c.len_utf16() as u32 > pos {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    s.len()
}

/// Byte offset `n` UTF-16 units before `from`, never splitting a character.
fn back_utf16(s: &str, from: usize, n: u32) -> usize {
    let mut units = 0u32;
    let mut at = from;
    for (i, c) in s[..from].char_indices().rev() {
        if units >= n {
            break;
        }
        units += c.len_utf16() as u32;
        at = i;
    }
    at
}

/// Byte offset `n` UTF-16 units after `from`, never splitting a character.
fn forward_utf16(s: &str, from: usize, n: u32) -> usize {
    let mut units = 0u32;
    let mut at = from;
    for c in s[from..].chars() {
        if units >= n {
            break;
        }
        units += c.len_utf16() as u32;
        at += c.len_utf8();
    }
    at
}

// android.text.InputType / EditorInfo constants.
const TYPE_CLASS_TEXT: i32 = 0x1;
const TYPE_CLASS_NUMBER: i32 = 0x2;
const TYPE_TEXT_FLAG_CAP_CHARACTERS: i32 = 0x1000;
const TYPE_TEXT_FLAG_CAP_WORDS: i32 = 0x2000;
const TYPE_TEXT_FLAG_CAP_SENTENCES: i32 = 0x4000;
const TYPE_TEXT_FLAG_AUTO_CORRECT: i32 = 0x8000;
const TYPE_TEXT_FLAG_NO_SUGGESTIONS: i32 = 0x80000;
const TYPE_TEXT_VARIATION_PASSWORD: i32 = 0x80;
const TYPE_NUMBER_FLAG_SIGNED: i32 = 0x1000;
const TYPE_NUMBER_FLAG_DECIMAL: i32 = 0x2000;
const IME_ACTION_SEARCH: i32 = 3;
const IME_ACTION_DONE: i32 = 6;

fn android_input_type(p: &InputMethodProperties) -> (i32, i32) {
    let hints = &p.input_method_hints;
    let mut text = TYPE_CLASS_TEXT;
    text |= match hints.capitalization {
        CapitalizationMode::Sentences => TYPE_TEXT_FLAG_CAP_SENTENCES,
        CapitalizationMode::Words => TYPE_TEXT_FLAG_CAP_WORDS,
        CapitalizationMode::Characters => TYPE_TEXT_FLAG_CAP_CHARACTERS,
        _ => 0,
    };
    if hints.auto_correct {
        text |= TYPE_TEXT_FLAG_AUTO_CORRECT;
    } else if !hints.auto_complete {
        text |= TYPE_TEXT_FLAG_NO_SUGGESTIONS;
    }
    match p.input_type {
        InputType::Password => (
            TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD,
            IME_ACTION_DONE,
        ),
        InputType::Number => (TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_SIGNED, IME_ACTION_DONE),
        InputType::Decimal => (
            TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_SIGNED | TYPE_NUMBER_FLAG_DECIMAL,
            IME_ACTION_DONE,
        ),
        InputType::Search => (text, IME_ACTION_SEARCH),
        _ => (text, IME_ACTION_DONE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets() {
        let s = "a😀é b";
        assert_eq!(utf16_len(s), 6);
        assert_eq!(byte_at_utf16(s, 0), 0);
        assert_eq!(byte_at_utf16(s, 1), 1);
        // Inside the surrogate pair rounds down to the emoji.
        assert_eq!(byte_at_utf16(s, 2), 1);
        assert_eq!(byte_at_utf16(s, 3), 5);
        assert_eq!(byte_at_utf16(s, 4), 7);
        assert_eq!(byte_at_utf16(s, 99), s.len());
        assert_eq!(back_utf16(s, 7, 1), 5);
        assert_eq!(back_utf16(s, 7, 3), 1);
        assert_eq!(back_utf16(s, 7, 99), 0);
        assert_eq!(forward_utf16(s, 1, 2), 5);
        assert_eq!(forward_utf16(s, 1, 1), 5, "never splits a surrogate pair");
        assert_eq!(forward_utf16(s, 7, 99), s.len());
    }

    fn focused(text: &str, cursor: usize) -> ImeState {
        let mut p = InputMethodProperties::default();
        p.text = text.into();
        p.cursor_position = cursor;
        let mut st = ImeState::default();
        st.request(InputMethodRequest::Enable(p));
        st
    }

    #[test]
    fn snapshot_inlines_composition() {
        let mut st = focused("hi there", 3);
        st.translate(ImeOp::Compose("wo".into()), 1);
        let (mode, snap, seq) = st.take_pending().unwrap();
        assert_eq!(mode, ImeMode::Show, "focus is still being announced");
        assert_eq!(seq, 1);
        assert_eq!(snap.text, "hi wothere");
        assert_eq!((snap.comp_start, snap.comp_end), (3, 5));
        assert_eq!((snap.sel_start, snap.sel_end), (5, 5));
        assert_eq!(snap.action, IME_ACTION_DONE);
        assert!(st.take_pending().is_none());
    }

    #[test]
    fn edits_without_focus_hide() {
        let mut st = ImeState::default();
        assert!(st.translate(ImeOp::Commit("x".into()), 4).is_empty());
        assert_eq!(st.take_pending().map(|p| p.0), Some(ImeMode::Hide));
    }

    #[test]
    fn modes_merge() {
        let mut st = focused("", 0);
        st.request(InputMethodRequest::Disable);
        let mut p = InputMethodProperties::default();
        p.text = "x".into();
        st.request(InputMethodRequest::Enable(p.clone()));
        st.request(InputMethodRequest::Update(p));
        assert_eq!(st.take_pending().map(|p| p.0), Some(ImeMode::Show));
        st.request(InputMethodRequest::Disable);
        assert_eq!(st.take_pending().map(|p| p.0), Some(ImeMode::Hide));
    }
}
