package com.mdhaarith.patch;

import android.accessibilityservice.AccessibilityService;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityEvent;
import android.view.inputmethod.InputMethodInfo;
import android.view.inputmethod.InputMethodManager;

import java.util.HashSet;
import java.util.Set;

/**
 * Short-video limit for every app in {@link #APPS}: whichever of them is installed is tracked
 * as soon as it opens, nothing to set up per app. Window changes (sent by Android for every app)
 * tell which app is open; its scroll events give videos swiped (ScrollCounter); feeds never count.
 * Shows a small counter under the camera while a tracked app is open and closes it at the daily
 * limit. Never reads window content.
 */
public class XScrollService extends AccessibilityService {
    static final Set<String> APPS = Set.of(
            "com.twitter.android",            // X
            "com.instagram.android",          // Reels
            "com.google.android.youtube",     // Shorts
            "com.zhiliaoapp.musically",       // TikTok
            "com.ss.android.ugc.trill",       // TikTok (Asia)
            "com.facebook.katana",            // Facebook Reels
            "com.snapchat.android",           // Spotlight
            "in.mohalla.video",               // Moj
            "com.eterno.shortvideos",         // Josh
            "com.instagram.barcelona");       // Threads

    private ScrollCounter counter = new ScrollCounter();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable endSwipe = () -> {
        counter.endSwipe();
        flush();
    };
    private final Set<String> overlays = new HashSet<>(); // packages whose windows open over an app without leaving it
    private XScrollOverlay overlay;
    private String app; // tracked app in front, or null
    private long since; // when time in it was last added

    @Override
    protected void onServiceConnected() {
        overlay = new XScrollOverlay(this);
        // Keyboards open over an app without leaving it, and closing one sends no event back.
        // shortcut: read once per connect; a keyboard enabled later is only ignored after the service restarts.
        for (InputMethodInfo im : getSystemService(InputMethodManager.class).getEnabledInputMethodList())
            overlays.add(im.getPackageName());
        overlays.add(getPackageName());         // our own counter and block screen
        overlays.add("com.android.systemui");   // notification shade, volume, recents
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent e) {
        CharSequence pkg = e.getPackageName();
        if (overlay == null || pkg == null) return;
        String p = pkg.toString();
        if (e.getEventType() == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) {
            if (!overlays.contains(p)) switchTo(APPS.contains(p) ? p : null);
            return;
        }
        if (e.getEventType() != AccessibilityEvent.TYPE_VIEW_SCROLLED || !APPS.contains(p)) return;
        // A listed app scrolling is in front, even if a popup from another app hid that.
        if (!p.equals(app)) switchTo(p);
        int dx = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P ? e.getScrollDeltaX() : -1; // -1 = not set
        int raw = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P ? e.getScrollDeltaY() : -1;
        if (!ScrollCounter.vertical(e.getFromIndex(), e.getMaxScrollY(), dx, raw)) return;
        counter.onScroll(e.getFromIndex(), e.getScrollY(), raw == -1 ? -1 : Math.abs(raw),
                getResources().getDisplayMetrics().heightPixels, SystemClock.elapsedRealtime());
        handler.removeCallbacks(endSwipe);
        handler.postDelayed(endSwipe, ScrollCounter.SWIPE_MS);
        if (counter.videos > 0) flush();
    }

    /** next = listed app now in front, or null for any other app. */
    private void switchTo(String next) {
        if (next == null ? app == null : next.equals(app)) return;
        handler.removeCallbacks(endSwipe);
        counter.endSwipe(); // keep a video swiped just before leaving
        if (app != null) flush(); // add the time and videos up to now
        app = next;
        counter = new ScrollCounter(); // positions of one app mean nothing in another
        since = SystemClock.elapsedRealtime();
        if (app != null) flush();
        else overlay.hide();
    }

    /** Stores pending totals and refreshes the counter or the block screen. */
    private void flush() {
        long now = SystemClock.elapsedRealtime();
        XScroll.add(this, counter.videos, app != null ? now - since : 0);
        counter.videos = 0;
        since = now;
        if (app == null) return;
        int limit = XScroll.limit(this);
        long[] t = XScroll.today(this);
        if (limit > 0 && t[2] >= limit) overlay.block(t, limit);
        else overlay.showCounter(t, limit);
    }

    @Override
    public void onInterrupt() {}
}
