package com.mdhaarith.patch;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Build;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.HashSet;
import java.util.Set;

/**
 * Delivers reminders through AlarmManager so they fire even after Android has
 * killed the app process. Rust hands over the full upcoming set whenever it
 * changes; it is persisted so a reboot can re-arm it.
 */
final class Reminders {
    private static final String TAG = "Patch";
    private static final String PREFS = "reminders";
    private static final String KEY_JSON = "json";
    private static final String KEY_CODES = "codes";
    private static final String KEY_KEYS = "keys";
    private static final String KEY_SNOOZES = "snoozes";
    private static final String KEY_DELIVERED = "delivered";
    static final String EXTRA_KEY = "key";
    static final String EXTRA_SNOOZED = "snoozed";
    static final String ACTION_DONE = "com.mdhaarith.patch.DONE";
    static final String ACTION_SNOOZE = "com.mdhaarith.patch.SNOOZE";
    static final String EXTRA_NOTIFICATION_ID = "notification_id";
    static final String EXTRA_TITLE = "title";
    static final String EXTRA_BODY = "body";

    private Reminders() {}

    /** Replaces every scheduled reminder with those in {@code json}. */
    static synchronized void schedule(Context context, String json) {
        SharedPreferences sp = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        sp.edit().putString(KEY_JSON, json).apply();
        arm(context, json);
    }

    /** Re-arms the last set after a reboot or an app update. */
    static synchronized void restore(Context context) {
        String json = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_JSON, "[]");
        arm(context, json);
    }

    static synchronized void snooze(Context context, Intent intent) {
        String key = intent.getStringExtra(EXTRA_KEY);
        if (key == null) return;
        try {
            SharedPreferences sp = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            JSONArray old = new JSONArray(sp.getString(KEY_SNOOZES, "[]"));
            JSONArray list = new JSONArray();
            long now = System.currentTimeMillis();
            for (int i = 0; i < old.length(); i++) {
                JSONObject r = old.getJSONObject(i);
                if (!key.equals(r.getString("key")) && r.getLong("at") > now) list.put(r);
            }
            list.put(new JSONObject().put("key", key)
                    .put("title", intent.getStringExtra(EXTRA_TITLE))
                    .put("body", intent.getStringExtra(EXTRA_BODY))
                    .put("at", now + 10 * 60 * 1000));
            sp.edit().putString(KEY_SNOOZES, list.toString()).apply();
            restore(context);
        } catch (JSONException e) {
            Log.w(TAG, "can't snooze reminder", e);
        }
    }

    /** Claims a delivery before posting; snoozes are independent of the original delivery. */
    static synchronized boolean delivered(Context context, String key, boolean snoozed) {
        SharedPreferences sp = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        Set<String> delivered = new HashSet<>(sp.getStringSet(KEY_DELIVERED, new HashSet<>()));
        if (!snoozed && delivered.contains(key)) return false;
        delivered.add(key);
        sp.edit().putStringSet(KEY_DELIVERED, delivered).apply();
        return true;
    }

    private static void arm(Context context, String json) {
        AlarmManager am = context.getSystemService(AlarmManager.class);
        SharedPreferences sp = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        for (String code : sp.getStringSet(KEY_CODES, new HashSet<>())) {
            PendingIntent pi = pending(context, Integer.parseInt(code), null, null, null, false);
            am.cancel(pi);
            pi.cancel();
        }
        for (String key : sp.getStringSet(KEY_KEYS, new HashSet<>())) {
            PendingIntent pi = pending(context, key.hashCode(), key, null, null, false);
            am.cancel(pi);
            pi.cancel();
        }
        Set<String> keys = new HashSet<>();
        long now = System.currentTimeMillis();
        try {
            JSONArray list = new JSONArray(json);
            int upcoming = list.length();
            Set<String> delivered = new HashSet<>(sp.getStringSet(KEY_DELIVERED, new HashSet<>()));
            Set<String> current = new HashSet<>();
            // Rust replaces upcoming alarms as tasks change; keep user snoozes separately.
            JSONArray snoozes = new JSONArray(sp.getString(KEY_SNOOZES, "[]"));
            JSONArray kept = new JSONArray();
            for (int i = 0; i < snoozes.length(); i++) {
                JSONObject r = snoozes.getJSONObject(i);
                if (r.getLong("at") > now) {
                    kept.put(r);
                    list.put(r);
                }
            }
            sp.edit().putString(KEY_SNOOZES, kept.toString()).apply();
            for (int i = 0; i < list.length(); i++) {
                JSONObject r = list.getJSONObject(i);
                String key = r.getString("key");
                current.add(key);
                boolean snoozed = i >= upcoming;
                if (!snoozed && delivered.contains(key)) continue;
                long at = Math.max(now, r.getLong("at"));
                PendingIntent pi = pending(context, key.hashCode(), key, r.getString("title"), r.getString("body"), snoozed);
                // Exact alarms need the user's (or the calendar-app) grant;
                // otherwise Android may delay delivery by a few minutes.
                if (Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms()) {
                    am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi);
                } else {
                    am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi);
                }
                keys.add(key);
            }
            delivered.retainAll(current);
            sp.edit().putStringSet(KEY_DELIVERED, delivered).apply();
        } catch (JSONException | SecurityException e) {
            Log.w(TAG, "can't schedule reminders", e);
        }
        sp.edit().remove(KEY_CODES).putStringSet(KEY_KEYS, keys).apply();
    }

    private static PendingIntent pending(Context context, int code, String key, String title, String body, boolean snoozed) {
        Intent intent = new Intent(context, ReminderReceiver.class);
        if (key != null) {
            intent.setData(new Uri.Builder().scheme("patch-reminder").authority("alarm").appendPath(key).build());
        }
        if (title != null) {
            intent.putExtra(EXTRA_KEY, key).putExtra(EXTRA_TITLE, title).putExtra(EXTRA_BODY, body)
                    .putExtra(EXTRA_SNOOZED, snoozed);
        }
        return PendingIntent.getBroadcast(context, code, intent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }
}
