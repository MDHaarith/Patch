package com.mdhaarith.patch;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.graphics.Rect;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.inputmethod.InputMethodInfo;
import android.view.inputmethod.InputMethodManager;

import java.util.HashSet;
import java.util.Set;

/**
 * Short-video limit for every app in {@link #APPS}: whichever of them is installed is tracked
 * as soon as it opens, nothing to set up per app. Window changes (sent by Android for every app)
 * tell which app is open; its scroll events give videos swiped (ScrollCounter); feeds never count.
 * Shows a small counter under the camera while a tracked app is open and closes it at the daily
 * limit. Uses control actions and bounds in X, never post text.
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

    private static final boolean TRACE = BuildConfig.APPLICATION_ID.endsWith(".test");
    private static final java.util.ArrayDeque<String> trace = new java.util.ArrayDeque<>();
    private static int contentEvents;

    private static void record(String line) {
        if (trace.size() == 120) trace.removeFirst();
        trace.addLast(SystemClock.elapsedRealtime() + " " + line);
    }

    static String swipeReport(android.content.Context context) {
        return "Patch swipe trace v2, Android " + Build.VERSION.SDK_INT + ", service enabled="
                + XScroll.serviceOn(context) + ", X content events=" + contentEvents + "\n"
                + String.join("\n", trace);
    }

    private final ScrollCounter.AppSessions sessions = new ScrollCounter.AppSessions();
    private ScrollCounter counter = new ScrollCounter();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable endSwipe = () -> {
        counter.endSwipe();
        flush();
    };
    private final Set<String> overlays = new HashSet<>(); // packages whose windows open over an app without leaving it
    private XScrollOverlay overlay;
    private static final String X = "com.twitter.android";
    private final Runnable inspectViewer = this::inspectViewer;
    private String app; // tracked app in front, or null
    private long since; // when time in it was last added

    @Override
    protected void onServiceConnected() {
        overlay = new XScrollOverlay(this);
        if (TRACE) record("service connected");
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
            if (X.equals(p)) scheduleInspection();
            return;
        }
        if (e.getEventType() == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED && X.equals(app) && X.equals(p)) {
            if (TRACE) contentEvents++;
            scheduleInspection();
            return;
        }
        boolean selected = e.getEventType() == AccessibilityEvent.TYPE_VIEW_SELECTED;
        if (!APPS.contains(p) || (selected && !X.equals(p))) return;
        if (!selected && e.getEventType() != AccessibilityEvent.TYPE_VIEW_SCROLLED) return;
        // A listed app scrolling is in front, even if a popup from another app hid that.
        if (!p.equals(app)) switchTo(p);
        boolean pagerSource = false;
        if (X.equals(p)) {
            AccessibilityNodeInfo source = e.getSource();
            if (source != null) {
                int height = pagerHeight(source);
                source.recycle();
                pagerSource = height > 0;
                if (pagerSource) {
                    xPagerDetected = true;
                    counter.setViewer(height);
                }
            }
        }
        int dx = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P ? e.getScrollDeltaX() : -1; // -1 = not set
        int raw = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P ? e.getScrollDeltaY() : -1;
        if (TRACE && X.equals(p)) record((selected ? "select" : "scroll") + " pagerSource=" + pagerSource
                + " viewer=" + counter.inViewer() + " from=" + e.getFromIndex() + " to=" + e.getToIndex()
                + " current=" + e.getCurrentItemIndex() + " y=" + e.getScrollY() + " max=" + e.getMaxScrollY()
                + " dx=" + dx + " dy=" + raw);
        if (selected) {
            if (pagerSource && !counter.hasPixelPager()) {
                int index = e.getCurrentItemIndex();
                counter.onViewerScroll(index >= 0 ? index : e.getFromIndex(), -1, -1);
                if (counter.videos > 0) flush();
            }
            return;
        }
        if (!ScrollCounter.acceptScroll(X.equals(p) && xPagerDetected, pagerSource,
                e.getFromIndex(), e.getMaxScrollY(), dx, raw)) {
            if (TRACE && X.equals(p)) record("ignored non-pager or nonvertical scroll");
            return;
        }
        boolean pixelPosition = pagerSource && e.getFromIndex() < 0
                && counter.onPagerPosition(e.getScrollY(), e.getMaxScrollY());
        if (!pixelPosition) {
            if (counter.hasPixelPager()) return; // a nested control has its own scroll coordinates
            counter.onScroll(e.getFromIndex(), e.getScrollY(), raw,
                    getResources().getDisplayMetrics().heightPixels, SystemClock.elapsedRealtime());
        }
        handler.removeCallbacks(endSwipe);
        handler.postDelayed(endSwipe, ScrollCounter.SWIPE_MS);
        if (counter.videos > 0) flush();
    }

    private boolean xPagerDetected;
    private boolean inspectionPending;
    private boolean pagerMissing;
    private boolean viewerCovered;

    private void scheduleInspection() {
        if (inspectionPending) return;
        inspectionPending = true;
        handler.postDelayed(inspectViewer, 100);
    }

    private void inspectViewer() {
        inspectionPending = false;
        if (!X.equals(app)) return;
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null) return;
        try {
            if (!X.equals(root.getPackageName())) return;
            int height = findPager(root);
            if (height > 0) xPagerDetected = true;
            int cover = height == 0 && counter.inViewer() ? viewerCover(root) : 0;
            if (cover != 0) {
                if (TRACE && !viewerCovered) record("viewer covered: " + (cover == 1 ? "dismissible panel" : "bottom editor"));
                viewerCovered = true;
                pagerMissing = false;
                return;
            }
            if (viewerCovered && TRACE) record(height > 0 ? "viewer uncovered" : "cover gone, pager absent");
            viewerCovered = false;
            if (height == 0 && counter.inViewer() && !pagerMissing) {
                // A Compose layout can briefly disappear while the page changes.
                pagerMissing = true;
                scheduleInspection();
                return;
            }
            pagerMissing = false;
            if (TRACE && counter.inViewer() != (height > 0)) record("viewer height=" + height);
            counter.setViewer(height);
            if (counter.videos > 0) flush();
        } finally {
            root.recycle();
        }
    }

    private int findPager(AccessibilityNodeInfo node) {
        int height = pagerHeight(node);
        for (int i = 0; height == 0 && i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child != null) {
                height = findPager(child);
                child.recycle();
            }
        }
        return height;
    }

    // A modal sheet can hide the pager from accessibility until the reader dismisses it.
    private int viewerCover(AccessibilityNodeInfo node) {
        boolean dismissible = node.isDismissable() || node.getActionList().contains(
                AccessibilityNodeInfo.AccessibilityAction.ACTION_DISMISS);
        if (node.isVisibleToUser() && (dismissible || node.isEditable())) {
            Rect bounds = new Rect();
            node.getBoundsInScreen(bounds);
            android.util.DisplayMetrics display = getResources().getDisplayMetrics();
            if (bounds.width() >= display.widthPixels * 0.85f && bounds.height() >= display.heightPixels / 4
                    && bounds.bottom >= display.heightPixels * 0.85f && dismissible) return 1;
            if (node.isEditable() && ScrollCounter.isBottomEditor(bounds.width(), bounds.top,
                    bounds.bottom, display.widthPixels, display.heightPixels)) return 2;
        }
        for (int i = 0; i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child == null) continue;
            int found = viewerCover(child);
            child.recycle();
            if (found != 0) return found;
        }
        return 0;
    }

    private int pagerHeight(AccessibilityNodeInfo node) {
        if (Build.VERSION.SDK_INT < 29 || !node.isVisibleToUser()) return 0;
        for (AccessibilityNodeInfo.AccessibilityAction action : node.getActionList()) {
            if (action.getId() != android.R.id.accessibilityActionPageDown
                    && action.getId() != android.R.id.accessibilityActionPageUp) continue;
            Rect bounds = new Rect();
            node.getBoundsInScreen(bounds);
            android.util.DisplayMetrics display = getResources().getDisplayMetrics();
            if (bounds.width() >= display.widthPixels * 0.85f && bounds.height() >= display.heightPixels * 0.75f)
                return bounds.height();
        }
        return 0;
    }

    /** next = listed app now in front, or null for any other app. */
    private void switchTo(String next) {
        if (next == null ? app == null : next.equals(app)) return;
        handler.removeCallbacks(endSwipe);
        counter.endSwipe(); // keep a video swiped just before leaving
        if (app != null) flush(); // add the time and videos up to now
        if (X.equals(app) || X.equals(next)) {
            handler.removeCallbacks(inspectViewer);
            inspectionPending = false;
            pagerMissing = false;
            viewerCovered = false;
            AccessibilityServiceInfo info = getServiceInfo();
            if (X.equals(next)) info.eventTypes |= AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED | AccessibilityEvent.TYPE_VIEW_SELECTED;
            else info.eventTypes &= ~(AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED | AccessibilityEvent.TYPE_VIEW_SELECTED);
            setServiceInfo(info);
        }
        if (TRACE && X.equals(next)) record("enter X");
        if (TRACE && X.equals(app)) record("leave X");
        app = next;
        counter = sessions.enter(X.equals(next));
        since = SystemClock.elapsedRealtime();
        if (app != null) flush();
        else overlay.hide();
    }

    /** Stores pending totals and refreshes the counter or the block screen. */
    private void flush() {
        long now = SystemClock.elapsedRealtime();
        long added = counter.videos;
        XScroll.add(this, added, app != null ? now - since : 0);
        counter.videos = 0;
        since = now;
        if (app == null) return;
        int limit = XScroll.limit(this);
        long[] t = XScroll.today(this);
        if (TRACE && X.equals(app) && added > 0) record("count +" + added + " daily total=" + t[2]);
        if (limit > 0 && t[2] >= limit) overlay.block(t, limit);
        else overlay.showCounter(t, limit);
    }

    @Override
    public void onInterrupt() {}
}
