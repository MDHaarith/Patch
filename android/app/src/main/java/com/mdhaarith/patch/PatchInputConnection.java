package com.mdhaarith.patch;

import android.text.Editable;
import android.text.Selection;
import android.view.inputmethod.BaseInputConnection;

/**
 * The IME's view of the focused Slint text field. {@link BaseInputConnection}
 * applies each edit to the mirror (so queries like getTextBeforeCursor see it
 * at once) and the same edit is forwarded to Rust, which applies it to the
 * real field with matching semantics.
 */
final class PatchInputConnection extends BaseInputConnection {
    private final PatchSurfaceView view;

    PatchInputConnection(PatchSurfaceView view) {
        super(view, true);
        this.view = view;
    }

    @Override
    public Editable getEditable() {
        return view.editable;
    }

    @Override
    public boolean beginBatchEdit() {
        view.beginBatch();
        return true;
    }

    @Override
    public boolean endBatchEdit() {
        view.endBatch();
        return view.batchDepth() > 0;
    }

    @Override
    public boolean commitText(CharSequence text, int newCursorPosition) {
        view.beginBatch();
        super.commitText(text, newCursorPosition);
        view.imeEdit(PatchNative.IME_COMMIT, text.toString(), 0, 0);
        view.endBatch();
        return true;
    }

    @Override
    public boolean setComposingText(CharSequence text, int newCursorPosition) {
        view.beginBatch();
        super.setComposingText(text, newCursorPosition);
        view.imeEdit(PatchNative.IME_COMPOSE, text.toString(), 0, 0);
        view.endBatch();
        return true;
    }

    @Override
    public boolean finishComposingText() {
        view.beginBatch();
        super.finishComposingText();
        view.imeEdit(PatchNative.IME_FINISH, null, 0, 0);
        view.endBatch();
        return true;
    }

    @Override
    public boolean deleteSurroundingText(int beforeLength, int afterLength) {
        view.beginBatch();
        super.deleteSurroundingText(beforeLength, afterLength);
        view.imeEdit(PatchNative.IME_DELETE, null, beforeLength, afterLength);
        view.endBatch();
        return true;
    }

    @Override
    public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
        // Rust counts UTF-16 units, like deleteSurroundingText; convert
        // against the mirror before it changes.
        Editable text = view.editable;
        int a = Math.min(Selection.getSelectionStart(text), Selection.getSelectionEnd(text));
        int b = Math.max(Selection.getSelectionStart(text), Selection.getSelectionEnd(text));
        int compStart = getComposingSpanStart(text);
        int compEnd = getComposingSpanEnd(text);
        if (compStart >= 0 && compEnd >= 0) {
            a = Math.min(a, Math.min(compStart, compEnd));
            b = Math.max(b, Math.max(compStart, compEnd));
        }
        if (a < 0 || b < 0) {
            return false;
        }
        return deleteSurroundingText(a - offsetBy(text, a, -beforeLength), offsetBy(text, b, afterLength) - b);
    }

    private static int offsetBy(CharSequence text, int index, int codePoints) {
        try {
            return Character.offsetByCodePoints(text, index, codePoints);
        } catch (IndexOutOfBoundsException e) {
            return codePoints < 0 ? 0 : text.length();
        }
    }

    @Override
    public boolean setSelection(int start, int end) {
        view.beginBatch();
        super.setSelection(start, end);
        view.imeEdit(PatchNative.IME_SELECT, null, start, end);
        view.endBatch();
        return true;
    }

    @Override
    public boolean setComposingRegion(int start, int end) {
        view.beginBatch();
        super.setComposingRegion(start, end);
        view.imeEdit(PatchNative.IME_REGION, null, start, end);
        view.endBatch();
        return true;
    }

    @Override
    public boolean performEditorAction(int editorAction) {
        view.imeEdit(PatchNative.IME_ACTION, null, editorAction, 0);
        return true;
    }
}
