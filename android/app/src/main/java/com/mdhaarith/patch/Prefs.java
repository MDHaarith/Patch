package com.mdhaarith.patch;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * The few settings Java needs before (or without) the Rust side: whether to
 * restore the overlay after a reboot, and where the overlay sits. The app's
 * real preferences live in Rust; these mirror the overlay flags.
 */
final class Prefs {
    static final String OVERLAY = "overlay";
    static final String OVERLAY_BOOT = "overlay_boot";
    static final String OVERLAY_EXPANDED = "overlay_expanded";
    static final String BUBBLE_X = "bubble_x";
    static final String BUBBLE_Y = "bubble_y";

    private Prefs() {}

    static SharedPreferences get(Context context) {
        return context.getSharedPreferences("patch", Context.MODE_PRIVATE);
    }

    static boolean is(Context context, String key) {
        return get(context).getBoolean(key, false);
    }

    static void set(Context context, String key, boolean value) {
        get(context).edit().putBoolean(key, value).apply();
    }
}
