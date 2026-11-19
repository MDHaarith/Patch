package com.mdhaarith.patch;

import android.content.Context;
import android.content.SharedPreferences;
import android.provider.Settings;

import java.time.LocalDate;

/**
 * Daily X scrolling totals and the daily limit, in their own SharedPreferences
 * (keyed "yyyy-MM-dd" + field). Java owns them because XScrollService runs
 * without the Rust side; Rust reads them through {@link #summary}.
 */
final class XScroll {
    // "scrolls" replaced the old "px" key, so leftover pixel totals are ignored.
    private static final String[] FIELDS = {"scrolls", "posts", "reels", "ms"};

    private XScroll() {}

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences("x_scroll", Context.MODE_PRIVATE);
    }

    /** Adds to today's totals. */
    static void add(Context context, long videos, long ms) {
        String day = LocalDate.now().toString();
        SharedPreferences p = prefs(context);
        long[] v = {0, 0, videos, ms}; // scrolls and posts are no longer counted; videos use the reels slot
        SharedPreferences.Editor ed = p.edit();
        for (int i = 0; i < FIELDS.length; i++) {
            String k = day + FIELDS[i];
            ed.putLong(k, p.getLong(k, 0) + v[i]);
        }
        ed.apply();
    }

    /** {scrolls, posts, videos, ms} for one day. */
    static long[] day(Context context, LocalDate date) {
        String day = date.toString();
        SharedPreferences p = prefs(context);
        long[] v = new long[FIELDS.length];
        for (int i = 0; i < FIELDS.length; i++) v[i] = p.getLong(day + FIELDS[i], 0);
        return v;
    }

    static long[] today(Context context) {
        return day(context, LocalDate.now());
    }

    /** Daily limit on videos; 0 = off. */
    static int limit(Context context) {
        return prefs(context).getInt("limit", 0);
    }

    static final int LOCK_DAYS = 30;

    /** Days until the limit can change again; 0 = now. A limit stays fixed for LOCK_DAYS once set. */
    static int lockDays(Context context) {
        SharedPreferences p = prefs(context);
        // No set day = a limit from before the lock existed; it can be set once more.
        if (p.getInt("limit", 0) == 0 || !p.contains("limit_day")) return 0;
        // shortcut: wall clock, so moving the date forward unlocks early; use network time if that gets abused.
        long since = LocalDate.now().toEpochDay() - p.getLong("limit_day", 0);
        return (int) Math.max(0, LOCK_DAYS - since);
    }

    /** Ignored while locked, so the limit can't be raised or removed in a weak moment. */
    static void setLimit(Context context, int limit) {
        if (lockDays(context) > 0) return;
        prefs(context).edit()
                .putInt("limit", Math.max(0, limit))
                .putLong("limit_day", LocalDate.now().toEpochDay())
                .apply();
    }

    static boolean serviceOn(Context context) {
        String enabled = Settings.Secure.getString(context.getContentResolver(), Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        return enabled != null && enabled.contains(context.getPackageName() + "/");
    }

    /**
     * For Rust's Settings card, comma-separated ints:
     * {@code state,limit,posts,videos,scrolls,minutes,d0..d6,lockDays} where state is
     * 1 tracking off / 2 on, d0..d6 are videos per day (d6 = today) and lockDays is
     * days until the limit can change (0 = now).
     */
    static String summary(Context context) {
        long[] t = today(context);
        StringBuilder sb = new StringBuilder()
                .append(serviceOn(context) ? 2 : 1).append(',')
                .append(limit(context)).append(',')
                .append(t[1]).append(',')
                .append(t[2]).append(',')
                .append(t[0]).append(',')
                .append(t[3] / 60_000);
        LocalDate now = LocalDate.now();
        for (int i = 6; i >= 0; i--) {
            long[] d = day(context, now.minusDays(i));
            sb.append(',').append(d[2]);
        }
        return sb.append(',').append(lockDays(context)).toString();
    }
}
