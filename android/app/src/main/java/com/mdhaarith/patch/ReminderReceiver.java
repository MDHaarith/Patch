package com.mdhaarith.patch;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Posts a reminder when its alarm fires, whether or not the app is running. */
public final class ReminderReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String title = intent.getStringExtra(Reminders.EXTRA_TITLE);
        if (title == null) {
            return;
        }
        if (Reminders.ACTION_DONE.equals(intent.getAction())) {
            String key = intent.getStringExtra(Reminders.EXTRA_KEY);
            if (key == null) return;
            context.getSystemService(android.app.NotificationManager.class)
                    .cancel(intent.getIntExtra(Reminders.EXTRA_NOTIFICATION_ID, -1));
            PatchNative.init(context);
            PendingResult pending = goAsync();
            new Thread(() -> {
                try {
                    PatchNative.nativeCompleteTask(key.split("@", 2)[0]);
                } finally {
                    pending.finish();
                }
            }, "patch-reminder-done").start();
            return;
        }
        if (Reminders.ACTION_SNOOZE.equals(intent.getAction())) {
            Reminders.snooze(context, intent);
            context.getSystemService(android.app.NotificationManager.class)
                    .cancel(intent.getIntExtra(Reminders.EXTRA_NOTIFICATION_ID, -1));
            return;
        }
        String key = intent.getStringExtra(Reminders.EXTRA_KEY);
        if (key == null) return;
        PatchNative.init(context);
        PendingResult pending = goAsync();
        new Thread(() -> {
            try {
                boolean open = PatchNative.nativeTaskOpen(key.split("@", 2)[0]);
                if (Reminders.delivered(context, key, intent.getBooleanExtra(Reminders.EXTRA_SNOOZED, false)) && open) {
                    Notifications.createChannels(context);
                    Notifications.reminder(context, key, title, intent.getStringExtra(Reminders.EXTRA_BODY));
                }
            } finally {
                pending.finish();
            }
        }, "patch-reminder").start();
    }
}
