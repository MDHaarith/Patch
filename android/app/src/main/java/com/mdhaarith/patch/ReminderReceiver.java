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
        Notifications.createChannels(context);
        Notifications.reminder(context, title, intent.getStringExtra(Reminders.EXTRA_BODY));
    }
}
