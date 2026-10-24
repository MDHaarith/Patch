package com.mdhaarith.patch;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.drawable.Icon;

final class Notifications {
    static final String CHANNEL_REMINDERS = "reminders";
    static final String CHANNEL_OVERLAY = "overlay";
    static final int OVERLAY_ID = 1;

    private static final int ACCENT = 0xFF0A84FF;
    private static final int FIRST_REMINDER_ID = 100;

    private Notifications() {}

    static void createChannels(Context context) {
        NotificationManager nm = context.getSystemService(NotificationManager.class);
        NotificationChannel reminders = new NotificationChannel(
                CHANNEL_REMINDERS, context.getString(R.string.channel_reminders), NotificationManager.IMPORTANCE_HIGH);
        reminders.setDescription(context.getString(R.string.channel_reminders_desc));
        NotificationChannel overlay = new NotificationChannel(
                CHANNEL_OVERLAY, context.getString(R.string.channel_overlay), NotificationManager.IMPORTANCE_LOW);
        overlay.setDescription(context.getString(R.string.channel_overlay_desc));
        overlay.setShowBadge(false);
        nm.createNotificationChannel(reminders);
        nm.createNotificationChannel(overlay);
    }

    static PendingIntent openApp(Context context) {
        Intent intent = new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    /** Thread-safe: called from Rust's UI thread. */
    static void reminder(Context context, String title, String body) {
        reminder(context, null, title, body);
    }

    static void reminder(Context context, String key, String title, String body) {
        int id = nextReminderId(context);
        Notification.Builder builder = new Notification.Builder(context, CHANNEL_REMINDERS)
                .setSmallIcon(R.drawable.ic_stat_patch)
                .setColor(ACCENT)
                .setContentTitle(title)
                .setContentText(body)
                .setStyle(new Notification.BigTextStyle().bigText(body))
                .setCategory(Notification.CATEGORY_REMINDER)
                .setContentIntent(openApp(context))
                .setAutoCancel(true);
        if (key != null) {
            Intent done = new Intent(context, ReminderReceiver.class).setAction(Reminders.ACTION_DONE)
                    .setData(new android.net.Uri.Builder().scheme("patch-reminder").authority("done").appendPath(key).build())
                    .putExtra(Reminders.EXTRA_KEY, key).putExtra(Reminders.EXTRA_TITLE, title)
                    .putExtra(Reminders.EXTRA_NOTIFICATION_ID, id);
            PendingIntent donePi = PendingIntent.getBroadcast(context, (key + ":done").hashCode(), done,
                    PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            Intent snooze = new Intent(context, ReminderReceiver.class).setAction(Reminders.ACTION_SNOOZE)
                    .setData(new android.net.Uri.Builder().scheme("patch-reminder").authority("snooze").appendPath(key).build())
                    .putExtra(Reminders.EXTRA_KEY, key).putExtra(Reminders.EXTRA_TITLE, title)
                    .putExtra(Reminders.EXTRA_BODY, body).putExtra(Reminders.EXTRA_NOTIFICATION_ID, id);
            PendingIntent snoozePi = PendingIntent.getBroadcast(context, (key + ":snooze").hashCode(), snooze,
                    PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            builder.addAction(new Notification.Action.Builder(null, "Done", donePi).build());
            builder.addAction(new Notification.Action.Builder(null, "Snooze 10 min", snoozePi).build());
        }
        Notification n = builder.build();
        // Without POST_NOTIFICATIONS this is silently dropped.
        context.getSystemService(NotificationManager.class).notify(id, n);
    }

    /** Persisted so reminders still in the tray aren't replaced after the
     * process is killed and the sequence would otherwise restart. */
    private static synchronized int nextReminderId(Context context) {
        SharedPreferences sp = context.getSharedPreferences("notifications", Context.MODE_PRIVATE);
        int id = sp.getInt("next_id", FIRST_REMINDER_ID);
        int next = id == Integer.MAX_VALUE ? FIRST_REMINDER_ID : id + 1;
        sp.edit().putInt("next_id", next).apply();
        return id;
    }

    static Notification overlay(Context context) {
        Intent hide = new Intent(context, OverlayService.class).setAction(OverlayService.ACTION_HIDE);
        PendingIntent hideIntent = PendingIntent.getService(context, 1, hide, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(context, CHANNEL_OVERLAY)
                .setSmallIcon(R.drawable.ic_stat_patch)
                .setColor(ACCENT)
                .setContentTitle(context.getString(R.string.overlay_title))
                .setContentText(context.getString(R.string.overlay_text))
                .setContentIntent(openApp(context))
                .addAction(new Notification.Action.Builder(
                        Icon.createWithResource(context, R.drawable.ic_stat_patch), context.getString(R.string.overlay_hide), hideIntent).build())
                .setCategory(Notification.CATEGORY_SERVICE)
                .setOngoing(true)
                .setShowWhen(false)
                .build();
    }
}
