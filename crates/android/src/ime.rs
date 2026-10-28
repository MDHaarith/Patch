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
    /// Text plus Android's `newCursorPosition`: 1 puts the cursor right
    /// after the text, >1 further right, <=0 relative to its start.
    Commit(String, i32),
    Compose(String, i32),
    FinishComposing,
    DeleteSurrounding { before: u32, after: u32 },
    SetSelection { start: u32, end: u32 },
    SetComposingRegion { start: u32, end: u32 },
    EditorAction,
}

impl ImeOp {
    pub fn from_java(op: i32, text: String, a: i32, b: i32) -> Option<ImeOp> {
        let new_cursor = a;
        let (a, b) = (a.max(0) as u32, b.max(0) as u32);
        Some(match op {
            0 => ImeOp::Commit(text, new_cursor),
            1 => ImeOp::Compose(text, new_cursor),
            2 => ImeOp::FinishComposing,
            3 => ImeOp::DeleteSurrounding {
                before: a,
                after: b,
            },
            4 => ImeOp::SetSelection {
                start: a,
                end: b,
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
    /// A long press in an editable field: show its text action bar too.
    ShowActions = 3,
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
    /// Selection relative to the composition (UTF-16 offsets), for the
    /// composition it was set with; otherwise the cursor sits after it.
    preedit_selection: Option<(String, std::ops::Range<i32>)>,
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
                self.pending = Some(self.pending.map_or(ImeMode::Show, |m| m.max(ImeMode::Show)));
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
        let c16 = utf16_len(&text[..cursor]) as i32;
        let len16 = utf16_len(text) as i32 + pre16;
        let selection = self
            .preedit_selection
            .as_ref()
            .filter(|(for_text, _)| !pre.is_empty() && for_text == pre)
            .map(|(_, range)| {
                (
                    (c16 + range.end).clamp(0, len16),
                    (c16 + range.start).clamp(0, len16),
                )
            });
        let (cur16, anc16) = match selection {
            Some(selection) => selection,
            None => (
                at(cursor),
                if anchor == cursor {
                    at(cursor)
                } else {
                    at(anchor)
                },
            ),
        };
        let (input_type, action) = android_input_type(p);
        Some(ImeSnapshot {
            text: format!("{}{}{}", &text[..cursor], pre, &text[cursor..]),
            sel_start: anc16,
            sel_end: cur16,
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

    pub fn show_actions(&mut self) {
        if self.props.is_some() {
            self.pending = Some(ImeMode::ShowActions);
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
            ImeOp::Commit(t, new_cursor) => {
                // The commit replaces the composition, or else the selection.
                let (start, end) = if self.preedit.is_empty() {
                    (anchor.min(cursor), anchor.max(cursor))
                } else {
                    (cursor, cursor)
                };
                self.preedit.clear();
                out.push(composition(
                    KeyEventType::CommitComposition,
                    &t,
                    "",
                    // An empty commit must delete the selection too (Cut).
                    Some(rel(start, cursor)..rel(end, cursor)),
                    None,
                ));
                // Slint leaves the cursor after the text; move it if the
                // IME asked for another spot (e.g. inside "()").
                if new_cursor != 1 {
                    let full = format!("{}{}{}", &text[..start], t, &text[end..]);
                    let start16 = utf16_len(&text[..start]) as i64;
                    let target = if new_cursor > 0 {
                        start16 + utf16_len(&t) as i64 + new_cursor as i64 - 1
                    } else {
                        start16 + new_cursor as i64
                    };
                    let target = target.clamp(0, utf16_len(&full) as i64) as u32;
                    let b = byte_at_utf16(&full, target) as i32;
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        "",
                        None,
                        Some((b, b)),
                    ));
                }
            }
            ImeOp::Compose(t, new_cursor) => {
                // Without a composition, composing replaces the selection.
                let range = (self.preedit.is_empty() && anchor != cursor)
                    .then(|| rel(anchor.min(cursor), cursor)..rel(anchor.max(cursor), cursor));
                // Slint can place a caret inside the composition but not
                // outside it; other requests keep the caret at its end.
                let len16 = utf16_len(&t) as i64;
                let k = if new_cursor > 0 {
                    len16 + new_cursor as i64 - 1
                } else {
                    new_cursor as i64
                };
                self.preedit_selection =
                    (0..len16).contains(&k).then(|| (t.clone(), k as i32..k as i32));
                let caret = self.preedit_selection.as_ref().map(|(_, range)| {
                    let b = byte_at_utf16(&t, range.end as u32) as i32;
                    b..b
                });
                self.preedit = t;
                out.push(composition_with_caret(
                    KeyEventType::UpdateComposition,
                    &self.preedit,
                    range,
                    caret,
                ));
            }
            ImeOp::FinishComposing => {
                if !self.preedit.is_empty() {
                    let snapshot = self.snapshot().unwrap();
                    let t = std::mem::take(&mut self.preedit);
                    out.push(composition(
                        KeyEventType::CommitComposition,
                        &t,
                        "",
                        None,
                        Some((
                            byte_at_utf16(&snapshot.text, snapshot.sel_end as u32) as i32,
                            byte_at_utf16(&snapshot.text, snapshot.sel_start as u32) as i32,
                        )),
                    ));
                }
            }
            ImeOp::DeleteSurrounding { before, after } => {
                // Android preserves the selection/composition and deletes on
                // either side. Work right-to-left so byte offsets stay valid.
                let (lo, hi) = (anchor.min(cursor), anchor.max(cursor));
                let start = back_utf16(&text, lo, before);
                let end = forward_utf16(&text, hi, after);
                let mut at = cursor;
                for (from, to) in [(hi, end), (start, lo)] {
                    if from == to {
                        continue;
                    }
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        &self.preedit,
                        Some(rel(from, at)..rel(to, at)),
                        None,
                    ));
                    at = from;
                }
                if !out.is_empty() {
                    let removed = lo - start;
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        &self.preedit,
                        None,
                        Some(((cursor - removed) as i32, (anchor - removed) as i32)),
                    ));
                }
            }
            ImeOp::SetSelection { start, end } => {
                let full = format!("{}{}{}", &text[..cursor], self.preedit, &text[cursor..]);
                let (s, e) = (byte_at_utf16(&full, start), byte_at_utf16(&full, end));
                if self.preedit.is_empty() {
                    out.push(composition(
                        KeyEventType::UpdateComposition,
                        "",
                        "",
                        None,
                        Some((e as i32, s as i32)),
                    ));
                } else {
                    let c16 = utf16_len(&text[..cursor]) as i32;
                    self.preedit_selection = Some((
                        self.preedit.clone(),
                        utf16_len(&full[..s]) as i32 - c16..utf16_len(&full[..e]) as i32 - c16,
                    ));
                    // Slint renders selection inside preedit; Android also needs positions outside it.
                    let caret = |b: usize| b.saturating_sub(cursor).min(self.preedit.len()) as i32;
                    out.push(composition_with_caret(
                        KeyEventType::UpdateComposition,
                        &self.preedit,
                        None,
                        Some(caret(s)..caret(e)),
                    ));
                }
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
        self.preedit_selection = None;
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

fn composition_with_caret(
    kind: KeyEventType,
    preedit: &str,
    replacement_range: Option<std::ops::Range<i32>>,
    caret: Option<std::ops::Range<i32>>,
) -> WindowEvent {
    let ev = InternalKeyEvent {
        event_type: kind,
        preedit_text: preedit.into(),
        replacement_range,
        preedit_selection: caret,
        ..Default::default()
    };
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

    fn key(ev: &WindowEvent) -> &InternalKeyEvent {
        match ev {
            WindowEvent::Internal(b) => match &**b {
                i_slint_core::platform::InternalEvent::Key(k) => k,
                _ => panic!("not a key event"),
            },
            _ => panic!("not an internal event"),
        }
    }

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
        st.translate(ImeOp::Compose("wo".into(), 1), 1);
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
    fn commit_honours_new_cursor_position() {
        // <= 0 counts from the start of the inserted text.
        let mut st = focused("ab", 1);
        let ev = st.translate(ImeOp::Commit("()".into(), 0), 2);
        assert_eq!(ev.len(), 2, "commit, then move the cursor");
        let k = key(&ev[1]);
        assert_eq!((k.cursor_position, k.anchor_position), (Some(1), Some(1)), "a|()b");
        let mut st = focused("ab", 1);
        let ev = st.translate(ImeOp::Commit("xy".into(), -1), 2);
        assert_eq!(key(&ev[1]).cursor_position, Some(0), "|axyb");

        let mut st = focused("ab", 1);
        let ev = st.translate(ImeOp::Commit("xy".into(), 2), 3);
        assert_eq!(key(&ev[1]).cursor_position, Some(4), "one past the inserted text: axyb|");

        let mut st = focused("ab", 1);
        assert_eq!(st.translate(ImeOp::Commit("xy".into(), 1), 4).len(), 1, "the default needs no move");
    }

    #[test]
    fn selection_direction_and_empty_commit() {
        assert_eq!(
            ImeOp::from_java(4, String::new(), 5, 1),
            Some(ImeOp::SetSelection { start: 5, end: 1 })
        );
        let mut st = focused("a😀bc", 1);
        st.props.as_mut().unwrap().anchor_position = Some(6);
        let (_, snapshot, _) = st.take_pending().unwrap();
        assert_eq!((snapshot.sel_start, snapshot.sel_end), (4, 1));
        let events = st.translate(ImeOp::Commit(String::new(), 1), 1);
        assert_eq!(
            key(&events[0]).replacement_range,
            Some(0..5),
            "empty commit deletes the selected bytes"
        );
        let events = st.translate(ImeOp::SetSelection { start: 4, end: 1 }, 2);
        assert_eq!(
            (
                key(&events[0]).anchor_position,
                key(&events[0]).cursor_position
            ),
            (Some(6), Some(1))
        );
    }

    #[test]
    fn compose_caret_inside_the_composition() {
        let mut st = focused("hi there", 3);
        st.translate(ImeOp::Compose("wor".into(), 0), 1);
        let (_, snap, _) = st.take_pending().unwrap();
        assert_eq!(snap.text, "hi worthere");
        assert_eq!((snap.sel_start, snap.sel_end), (3, 3), "caret at the composition's start");
        st.translate(ImeOp::Compose("word".into(), 1), 2);
        let (_, snap, _) = st.take_pending().unwrap();
        assert_eq!((snap.sel_start, snap.sel_end), (7, 7), "back to the end once the IME asks for it");
    }

    #[test]
    fn selection_preserves_composition() {
        for (start, end) in [(0, 0), (2, 2), (4, 2), (5, 5)] {
            let mut st = focused("ab", 1);
            st.translate(ImeOp::Compose("hel".into(), 1), 1);
            let events = st.translate(ImeOp::SetSelection { start, end }, 2);
            assert_eq!(events.len(), 1, "selection must not commit preedit");
            assert_eq!(key(&events[0]).preedit_text, "hel");
            let (_, snap, _) = st.take_pending().unwrap();
            assert_eq!(snap.text, "ahelb");
            assert_eq!((snap.comp_start, snap.comp_end), (1, 4));
            assert_eq!((snap.sel_start, snap.sel_end), (start as i32, end as i32));
            let events = st.translate(ImeOp::Commit("hello".into(), 1), 3);
            assert_eq!(events.len(), 1);
            assert_eq!(key(&events[0]).key_event.text, "hello");
            assert_eq!(key(&events[0]).replacement_range, Some(0..0));
        }
        let mut st = focused("ab", 1);
        st.translate(ImeOp::Compose("hel".into(), 1), 1);
        st.translate(ImeOp::SetSelection { start: 2, end: 2 }, 2);
        let events = st.translate(ImeOp::FinishComposing, 3);
        assert_eq!(key(&events[0]).key_event.text, "hel");
        assert_eq!((key(&events[0]).cursor_position, key(&events[0]).anchor_position), (Some(2), Some(2)));
    }

    #[test]
    fn edits_without_focus_hide() {
        let mut st = ImeState::default();
        assert!(st.translate(ImeOp::Commit("x".into(), 1), 4).is_empty());
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
        st.show_actions();
        st.request(InputMethodRequest::Enable(InputMethodProperties::default()));
        assert_eq!(st.take_pending().map(|p| p.0), Some(ImeMode::ShowActions));
        st.request(InputMethodRequest::Disable);
        assert_eq!(st.take_pending().map(|p| p.0), Some(ImeMode::Hide));
    }
}
