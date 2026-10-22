package com.mdhaarith.patch;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
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

    private static void arm(Context context, String json) {
        AlarmManager am = context.getSystemService(AlarmManager.class);
        SharedPreferences sp = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        for (String code : sp.getStringSet(KEY_CODES, new HashSet<>())) {
            am.cancel(pending(context, Integer.parseInt(code), null, null));
        }
        Set<String> codes = new HashSet<>();
        long now = System.currentTimeMillis();
        try {
            JSONArray list = new JSONArray(json);
            for (int i = 0; i < list.length(); i++) {
                JSONObject r = list.getJSONObject(i);
                long at = r.getLong("at");
                if (at < now - 60_000) {
                    continue;
                }
                int code = r.getString("key").hashCode();
                PendingIntent pi = pending(context, code, r.getString("title"), r.getString("body"));
                // Exact alarms need the user's (or the calendar-app) grant;
                // otherwise Android may delay delivery by a few minutes.
                if (Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms()) {
                    am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi);
                } else {
                    am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi);
                }
                codes.add(Integer.toString(code));
            }
        } catch (JSONException | SecurityException e) {
            Log.w(TAG, "can't schedule reminders", e);
        }
        sp.edit().putStringSet(KEY_CODES, codes).apply();
    }

    private static PendingIntent pending(Context context, int code, String title, String body) {
        Intent intent = new Intent(context, ReminderReceiver.class);
        if (title != null) {
            intent.putExtra(EXTRA_TITLE, title).putExtra(EXTRA_BODY, body);
        }
        return PendingIntent.getBroadcast(context, code, intent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }
}
