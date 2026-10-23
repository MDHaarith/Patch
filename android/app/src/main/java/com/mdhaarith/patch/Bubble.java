package com.mdhaarith.patch;

import android.annotation.TargetApi;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Person;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.drawable.Icon;
import android.os.Build;
import android.provider.Settings;
import android.util.Log;

/**
 * The floating panel on Android 11+: a system bubble showing {@link BubbleActivity}.
 * Unlike a TYPE_APPLICATION_OVERLAY window it needs no "display over other
 * apps" permission, so Android posts no "Patch is displaying over other apps"
 * notice, and no foreground service either. Its notification is suppressed;
 * only the bubble shows. Android 8–10 keep {@link OverlayService}.
 */
public final class Bubble extends BroadcastReceiver {
    static final String ACTION_HIDE = "com.mdhaarith.patch.action.HIDE_BUBBLE";
    private static final String CHANNEL = "bubble";
    private static final String SHORTCUT = "patch-bubble";
    private static final int ID = 2;
    private static final int HEIGHT_DP = 520;

    static boolean supported() {
        return Build.VERSION.SDK_INT >= 30;
    }

    /** Notifications on and bubbles not switched off for Patch. Under "Selected
     * conversations" the notification carries a bubble button the user taps. */
    @TargetApi(30)
    static boolean allowed(Context context) {
        NotificationManager nm = context.getSystemService(NotificationManager.class);
        if (!nm.areNotificationsEnabled()) {
            return false;
        }
        if (Build.VERSION.SDK_INT >= 31) {
            return nm.getBubblePreference() != NotificationManager.BUBBLE_PREFERENCE_NONE;
        }
        return areBubblesAllowed(nm);
    }

    @TargetApi(29)
    @SuppressWarnings("deprecation")
    private static boolean areBubblesAllowed(NotificationManager nm) {
        return nm.areBubblesAllowed();
    }

    /** Where the user lets Patch bubble ("All conversations can bubble"). */
    @TargetApi(30)
    static void openSettings(Context context) {
        Intent intent = new Intent(Settings.ACTION_APP_NOTIFICATION_BUBBLE_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, context.getPackageName())
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            context.startActivity(intent);
        } catch (RuntimeException e) {
            Log.w(PatchNative.TAG, "no bubble settings screen", e);
        }
    }

    @TargetApi(30)
    static void show(Context context) {
        NotificationManager nm = context.getSystemService(NotificationManager.class);
        NotificationChannel channel = new NotificationChannel(
                CHANNEL, context.getString(R.string.channel_bubble), NotificationManager.IMPORTANCE_DEFAULT);
        channel.setDescription(context.getString(R.string.channel_bubble_desc));
        channel.setAllowBubbles(true);
        channel.setSound(null, null);
        channel.setShowBadge(false);
        nm.createNotificationChannel(channel);

        // Bubbles are for conversations: a long-lived shortcut with a person.
        Icon icon = Icon.createWithResource(context, R.mipmap.ic_launcher);
        Person patch = new Person.Builder().setName(context.getString(R.string.app_name))
                .setIcon(icon).setImportant(true).build();
        ShortcutInfo shortcut = new ShortcutInfo.Builder(context, SHORTCUT)
                .setShortLabel(context.getString(R.string.app_name))
                .setLongLived(true)
                .setIcon(icon)
                .setPerson(patch)
                .setIntent(new Intent(context, MainActivity.class).setAction(Intent.ACTION_VIEW))
                .build();
        context.getSystemService(ShortcutManager.class).pushDynamicShortcut(shortcut);

        // The bubble's activity must be launched through a mutable intent.
        PendingIntent target = PendingIntent.getActivity(context, 2, new Intent(context, BubbleActivity.class),
                PendingIntent.FLAG_MUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification.BubbleMetadata bubble = new Notification.BubbleMetadata.Builder(target, icon)
                .setDesiredHeight(HEIGHT_DP)
                .setSuppressNotification(true)
                .build();
        PendingIntent hide = PendingIntent.getBroadcast(context, 3,
                new Intent(context, Bubble.class).setAction(ACTION_HIDE),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification n = new Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_patch)
                .setContentTitle(context.getString(R.string.app_name))
                .setContentText(context.getString(R.string.bubble_text))
                .setShortcutId(SHORTCUT)
                .addPerson(patch)
                .setStyle(new Notification.MessagingStyle(patch)
                        .addMessage(context.getString(R.string.bubble_text), System.currentTimeMillis(), patch))
                .setBubbleMetadata(bubble)
                .setCategory(Notification.CATEGORY_MESSAGE)
                .setContentIntent(target)
                .addAction(new Notification.Action.Builder(icon, context.getString(R.string.overlay_hide), hide).build())
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .build();
        nm.notify(ID, n);
    }

    static void hide(Context context) {
        context.getSystemService(NotificationManager.class).cancel(ID);
    }

    /** "Hide" on the notification (shown once the user drags the bubble away). */
    @Override
    public void onReceive(Context context, Intent intent) {
        if (ACTION_HIDE.equals(intent.getAction())) {
            hide(context);
            Prefs.set(context, Prefs.OVERLAY, false);
            PatchNative.init(context);
            PatchNative.nativeOverlayClosed();
        }
    }
}
