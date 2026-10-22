package com.mdhaarith.patch;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.text.format.DateFormat;
import android.util.Log;
import android.widget.RemoteViews;

import org.json.JSONException;
import org.json.JSONObject;

import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;

/**
 * The home-screen widget: the pet, today's date and open count, then today's
 * tasks and what's coming up (rows from {@link PatchWidgetService}).
 *
 * <p>Rust publishes a JSON snapshot whenever it changes
 * ({@link PatchNative#publishSnapshot}); it is stored so the widget renders
 * the same way when the process was started only for the widget and Rust
 * isn't running.
 */
public final class PatchWidget extends AppWidgetProvider {
    private static final String PREFS = "widget";
    private static final String KEY_JSON = "json";
    /** The local day ({@code yyyy-MM-dd}) the stored snapshot was made on. */
    private static final String KEY_DAY = "day";

    // PendingIntent request codes; extras don't make PendingIntents distinct.
    private static final int OPEN = 1;
    private static final int ADD = 2;
    private static final int ROW = 3;

    /** Stores a snapshot from Rust and redraws every widget. Any thread. */
    static void publish(Context context, String json) {
        prefs(context).edit().putString(KEY_JSON, json).putString(KEY_DAY, day()).apply();
        AppWidgetManager mgr = AppWidgetManager.getInstance(context);
        int[] ids = mgr.getAppWidgetIds(new ComponentName(context, PatchWidget.class));
        if (ids.length > 0) {
            update(context, mgr, ids);
        }
    }

    /**
     * The last snapshot, or an empty one. A snapshot from an earlier day is
     * dropped: its "today" is no longer today, and Rust isn't running to
     * redo the lists, so the widget asks to be opened instead.
     */
    static JSONObject snapshot(Context context) {
        String json = prefs(context).getString(KEY_JSON, null);
        if (json != null && !stale(context)) {
            try {
                return new JSONObject(json);
            } catch (JSONException e) {
                Log.w(PatchNative.TAG, "bad widget snapshot", e);
            }
        }
        return new JSONObject();
    }

    /** True when the stored snapshot was made on an earlier day. */
    static boolean stale(Context context) {
        String made = prefs(context).getString(KEY_DAY, null);
        return made != null && !made.equals(day());
    }

    @Override
    public void onUpdate(Context context, AppWidgetManager mgr, int[] ids) {
        update(context, mgr, ids);
    }

    @Override
    public void onAppWidgetOptionsChanged(Context context, AppWidgetManager mgr, int id, Bundle options) {
        update(context, mgr, new int[] {id});
    }

    @SuppressWarnings("deprecation") // setRemoteAdapter/notifyAppWidgetViewDataChanged: the service path works on every API.
    private static void update(Context context, AppWidgetManager mgr, int[] ids) {
        JSONObject snap = snapshot(context);
        for (int id : ids) {
            mgr.updateAppWidget(id, views(context, id, snap));
        }
        // The list re-reads the stored snapshot.
        mgr.notifyAppWidgetViewDataChanged(ids, R.id.widget_list);
    }

    @SuppressWarnings("deprecation")
    private static RemoteViews views(Context context, int id, JSONObject snap) {
        RemoteViews v = new RemoteViews(context.getPackageName(), R.layout.widget);

        boolean stale = stale(context);
        v.setImageViewResource(R.id.widget_pet, pet(stale ? "sleepy" : snap.optString("mood")));
        v.setTextViewText(R.id.widget_date, date(context, snap));
        int open = snap.optInt("open", 0);
        v.setTextViewText(R.id.widget_count, stale
                ? context.getString(R.string.widget_stale_count)
                : open == 0
                ? context.getString(R.string.widget_all_clear)
                : context.getResources().getQuantityString(R.plurals.widget_open, open, open));
        v.setTextViewText(R.id.widget_empty,
                context.getString(stale ? R.string.widget_stale : R.string.widget_empty));

        // The list: one adapter per widget id (the data URI makes the
        // intents distinct, so each widget gets its own factory).
        Intent service = new Intent(context, PatchWidgetService.class)
                .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id);
        service.setData(Uri.parse(service.toUri(Intent.URI_INTENT_SCHEME)));
        v.setRemoteAdapter(R.id.widget_list, service);
        v.setEmptyView(R.id.widget_list, R.id.widget_empty);

        v.setOnClickPendingIntent(R.id.widget_header, activity(context, OPEN, new Intent(), false));
        v.setOnClickPendingIntent(R.id.widget_empty, activity(context, OPEN, new Intent(), false));
        v.setOnClickPendingIntent(R.id.widget_add,
                activity(context, ADD, new Intent().putExtra(MainActivity.EXTRA_NEW_TASK, true), false));
        // Rows fill in the task id; collection fill-ins need a mutable template.
        v.setPendingIntentTemplate(R.id.widget_list, activity(context, ROW, new Intent(), true));
        return v;
    }

    private static PendingIntent activity(Context context, int code, Intent intent, boolean mutable) {
        intent.setClass(context, MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (!mutable) {
            flags |= PendingIntent.FLAG_IMMUTABLE;
        } else if (Build.VERSION.SDK_INT >= 31) {
            flags |= PendingIntent.FLAG_MUTABLE;
        }
        return PendingIntent.getActivity(context, code, intent, flags);
    }

    private static int pet(String mood) {
        switch (mood) {
            case "busy":
                return R.drawable.pet_busy;
            case "worried":
                return R.drawable.pet_worried;
            case "sleepy":
                return R.drawable.pet_sleepy;
            default:
                return R.drawable.pet_happy;
        }
    }

    /**
     * Rust's date while the snapshot is from today; otherwise (none yet, or
     * after midnight) the local date, so the header rolls over even before
     * the app runs again.
     */
    private static String date(Context context, JSONObject snap) {
        String stored = snap.optString("date");
        if (!stored.isEmpty()) { // only today's snapshot survives snapshot()
            return stored;
        }
        Locale locale = context.getResources().getConfiguration().getLocales().get(0);
        return new SimpleDateFormat(DateFormat.getBestDateTimePattern(locale, "EEEEdMMMM"), locale).format(new Date());
    }

    private static String day() {
        return new SimpleDateFormat("yyyy-MM-dd", Locale.ROOT).format(new Date());
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }
}
