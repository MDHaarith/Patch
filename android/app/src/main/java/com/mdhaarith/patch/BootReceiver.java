package com.mdhaarith.patch;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Re-arms reminders and brings the floating panel back (if asked to) after
 * a reboot or an app update. */
public final class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        boolean boot = Intent.ACTION_BOOT_COMPLETED.equals(action);
        boolean updated = Intent.ACTION_MY_PACKAGE_REPLACED.equals(action);
        if (!boot && !updated) {
            return;
        }
        // Alarms don't survive a reboot.
        Reminders.restore(context);
        if (!Prefs.is(context, Prefs.OVERLAY) || !PatchNative.floatAllowed(context)) {
            return;
        }
        if (boot && !Prefs.is(context, Prefs.OVERLAY_BOOT)) {
            return;
        }
        OverlayService.start(context);
    }
}
