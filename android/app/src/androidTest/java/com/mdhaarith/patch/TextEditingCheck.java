package com.mdhaarith.patch;

import android.app.Activity;
import android.app.AlarmManager;
import android.app.Instrumentation;
import android.app.PendingIntent;
import android.content.Context;
import android.content.ContextWrapper;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
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
                checkReminders();
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

    private void checkReminders() throws Exception {
        Context context = new ContextWrapper(getTargetContext()) {
            @Override
            public SharedPreferences getSharedPreferences(String name, int mode) {
                return super.getSharedPreferences("reminders-check", mode);
            }
        };
        SharedPreferences sp = context.getSharedPreferences("reminders", Context.MODE_PRIVATE);
        sp.edit().clear().commit();
        String a = "reminders-check-Aa@next";
        String b = "reminders-check-BB@next";
        String snoozeA = "reminders-check-Aa@old";
        String snoozeB = "reminders-check-BB@old";
        long at = System.currentTimeMillis() + 24 * 60 * 60 * 1000;
        org.json.JSONObject first = new org.json.JSONObject().put("key", a)
                .put("title", "check").put("body", "check").put("at", at);
        org.json.JSONObject second = new org.json.JSONObject(first.toString()).put("key", b);
        try {
            check(a.hashCode() == b.hashCode(), "fixture has colliding reminder hashes");
            Intent legacy = new Intent(context, ReminderReceiver.class);
            PendingIntent.getBroadcast(context, a.hashCode(), legacy,
                    PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            sp.edit().putStringSet("codes", java.util.Collections.singleton(Integer.toString(a.hashCode()))).commit();
            Reminders.schedule(context, new org.json.JSONArray().put(first).put(second).toString());
            check(PendingIntent.getBroadcast(context, a.hashCode(), legacy,
                    PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_NO_CREATE) == null,
                    "legacy hash-only reminder intents are cancelled");
            PendingIntent piA = reminderPending(context, a);
            PendingIntent piB = reminderPending(context, b);
            check(piA != null && piB != null && !piA.equals(piB), "colliding reminders have distinct intents");
            for (String key : new String[] { snoozeA, snoozeB }) {
                Reminders.snooze(context, new Intent().putExtra(Reminders.EXTRA_KEY, key)
                        .putExtra(Reminders.EXTRA_TITLE, "check").putExtra(Reminders.EXTRA_BODY, "check"));
            }
            check(new org.json.JSONArray(sp.getString("snoozes", "[]")).length() == 2,
                    "snoozes match tasks across occurrence keys");
            Reminders.restore(context);
            check(reminderPending(context, snoozeA) != null && reminderPending(context, snoozeB) != null,
                    "snoozes survive restore");
            Reminders.schedule(context, new org.json.JSONArray().put(first).toString());
            org.json.JSONArray kept = new org.json.JSONArray(sp.getString("snoozes", "[]"));
            check(kept.length() == 2, "leaving the upcoming window preserves snoozes");
            check(reminderPending(context, b) == null && reminderPending(context, snoozeB) != null,
                    "leaving the upcoming window cancels only the normal alarm");
            context.getSystemService(AlarmManager.class).cancel(reminderPending(context, a));
            reminderPending(context, a).cancel();
            new BootReceiver().onReceive(context,
                    new Intent(AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED));
            check(reminderPending(context, a) != null, "permission grant re-arms persisted reminders");
            Reminders.schedule(context, "[]");
            Reminders.restore(context);
            check(new org.json.JSONArray(sp.getString("snoozes", "[]")).length() == 2
                    && reminderPending(context, snoozeA) != null && reminderPending(context, snoozeB) != null,
                    "snoozes survive an empty upcoming list and restore");
            check(reminderPending(context, a) == null, "empty upcoming list cancels normal alarms");
            check(Reminders.delivered(context, snoozeA, false), "original delivery is recorded");
            Reminders.restore(context);
            check(reminderPending(context, snoozeA) != null, "delivered original does not suppress its snooze");
            check(Reminders.delivered(context, snoozeA, true), "snooze can deliver after its original");

            first.put("at", System.currentTimeMillis() - 60_000);
            Reminders.schedule(context, new org.json.JSONArray().put(first).toString());
            check(reminderPending(context, a) != null
                    && sp.getStringSet("keys", java.util.Collections.emptySet()).contains(a),
                    "late undelivered reminder is armed");
            check(Reminders.delivered(context, a, false), "late delivery is recorded");
            check(!Reminders.delivered(context, a, false), "duplicate normal delivery is suppressed");
            Reminders.restore(context);
            check(reminderPending(context, a) == null, "delivered reminder is not re-armed");
            check(sp.getStringSet("delivered", java.util.Collections.emptySet()).contains(snoozeA),
                    "delivered snooze keys survive pruning");
            sp.edit().putString("snoozes", new org.json.JSONArray().put(
                    new org.json.JSONObject(first.toString()).put("key", snoozeA)).toString()).commit();
            Reminders.schedule(context, "[]");
            check("[]".equals(sp.getString("snoozes", "[]")), "expired snoozes are pruned by time");
            check(sp.getStringSet("delivered", java.util.Collections.emptySet()).isEmpty(),
                    "delivered keys outside the latest list and snoozes are pruned");
        } finally {
            sp.edit().remove("snoozes").commit();
            Reminders.schedule(context, "[]");
            sp.edit().clear().commit();
        }
    }

    private static PendingIntent reminderPending(Context context, String key) {
        Intent intent = new Intent(context, ReminderReceiver.class)
                .setData(new Uri.Builder().scheme("patch-reminder").authority("alarm").appendPath(key).build());
        return PendingIntent.getBroadcast(context, key.hashCode(), intent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_NO_CREATE);
    }

    private static void check(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }
}
