package com.mdhaarith.patch;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.graphics.drawable.Icon;

import java.util.concurrent.atomic.AtomicInteger;

final class Notifications {
    static final String CHANNEL_REMINDERS = "reminders";
    static final String CHANNEL_OVERLAY = "overlay";
    static final int OVERLAY_ID = 1;

    private static final int ACCENT = 0xFF0A84FF;
    private static final AtomicInteger nextId = new AtomicInteger(100);

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
        Notification n = new Notification.Builder(context, CHANNEL_REMINDERS)
                .setSmallIcon(R.drawable.ic_stat_patch)
                .setColor(ACCENT)
                .setContentTitle(title)
                .setContentText(body)
                .setStyle(new Notification.BigTextStyle().bigText(body))
                .setCategory(Notification.CATEGORY_REMINDER)
                .setContentIntent(openApp(context))
                .setAutoCancel(true)
                .build();
        // Without POST_NOTIFICATIONS this is silently dropped.
        context.getSystemService(NotificationManager.class).notify(nextId.getAndIncrement(), n);
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
