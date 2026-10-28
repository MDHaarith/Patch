package com.mdhaarith.patch;

import android.app.AlarmManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.provider.Settings;

/** Re-arms reminders after a reboot, app update or exact-alarm grant, and
 * brings the floating panel back (if asked to) after a reboot or update. */
public final class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        boolean boot = Intent.ACTION_BOOT_COMPLETED.equals(action);
        boolean updated = Intent.ACTION_MY_PACKAGE_REPLACED.equals(action);
        boolean alarmGrant = AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED.equals(action);
        if (!boot && !updated && !alarmGrant) {
            return;
        }
        // Reboots and exact-alarm permission revocation cancel alarms.
        Reminders.restore(context);
        if (alarmGrant) return;
        if (!Prefs.is(context, Prefs.OVERLAY) || !Settings.canDrawOverlays(context)) {
            return;
        }
        if (boot && !Prefs.is(context, Prefs.OVERLAY_BOOT)) {
            return;
        }
        OverlayService.start(context);
    }
}
