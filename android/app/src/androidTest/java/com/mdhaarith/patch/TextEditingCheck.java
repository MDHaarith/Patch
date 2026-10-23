package com.mdhaarith.patch;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.text.Selection;

/** Device self-check, without a test framework: ./gradlew connectedDebugAndroidTest. */
public final class TextEditingCheck extends Instrumentation {
    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        start();
    }

    @Override
    public void onStart() {
        Bundle result = new Bundle();
        Bundle status = new Bundle();
        status.putString("class", getClass().getName());
        status.putString("test", "selectionAndClipboard");
        status.putInt("numtests", 1);
        status.putInt("current", 1);
        sendStatus(1, status);
        Throwable[] failure = new Throwable[1];
        Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        waitForIdleSync();
        runOnMainSync(() -> {
            try {
                PatchSurfaceView view = new PatchSurfaceView(activity, PatchNative.MAIN);
                PatchInputConnection input = new PatchInputConnection(view);
                view.editable.append("a😀bc");
                Selection.setSelection(view.editable, 4, 1);
                check("😀b".contentEquals(input.getSelectedText(0)), "reversed UTF-16 selection");
                check(!input.setSelection(-1, 0), "reject negative selection");
                check(!input.setSelection(0, 6), "reject selection past the text");
                check(input.performContextMenuAction(android.R.id.copy), "Copy handled");
                check("😀b".equals(PatchNative.clipboardText()), "Copy writes the system clipboard");
                check(input.performContextMenuAction(android.R.id.cut), "Cut handled");
                check("ac".contentEquals(view.editable), "Cut deletes the selection");
                check(input.getSelectedText(0) == null, "collapsed selection has no selected text");
                check(input.performContextMenuAction(android.R.id.paste), "Paste handled");
                check("a😀bc".contentEquals(view.editable), "Paste replaces at the cursor");
                check(input.performContextMenuAction(android.R.id.selectAll), "Select all handled");
                check("a😀bc".contentEquals(input.getSelectedText(0)), "Select all returns the full text");
                input.setComposingText("new", 1);
                input.commitText("two\nlines😀", 1);
                check("two\nlines😀".contentEquals(view.editable), "commit replaces composition and preserves newlines");
                check(Selection.getSelectionEnd(view.editable) == view.editable.length(), "commit cursor uses UTF-16");
                input.setSelection(3, 3);
                input.commitText("()", 0);
                check(Selection.getSelectionEnd(view.editable) == 3, "commit honors newCursorPosition");
                input.performContextMenuAction(android.R.id.selectAll);
                input.commitText("a😀bc!", 1);
                input.setSelection(4, 1);
                input.deleteSurroundingText(1, 1);
                check("😀b!".contentEquals(view.editable), "surrounding deletion preserves selected text");
                check(Selection.getSelectionStart(view.editable) == 3
                        && Selection.getSelectionEnd(view.editable) == 0, "surrounding deletion preserves selection direction");
            } catch (Throwable error) {
                failure[0] = error;
            }
        });
        runOnMainSync(activity::finish);
        if (failure[0] == null) {
            result.putString("stream", "Text editing checks passed\n");
            sendStatus(0, status);
            finish(Activity.RESULT_OK, result);
        } else {
            result.putString("stream", "Text editing check failed: " + failure[0] + "\n");
            status.putString("stack", failure[0].toString());
            sendStatus(-2, status);
            finish(Activity.RESULT_OK, result);
        }
    }

    private static void check(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }
}
