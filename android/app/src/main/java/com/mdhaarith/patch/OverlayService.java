package com.mdhaarith.patch;

import android.animation.ValueAnimator;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.content.res.Configuration;
import android.graphics.Outline;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.os.Build;
import android.os.IBinder;
import android.provider.Settings;
import android.util.DisplayMetrics;
import android.util.Log;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewOutlineProvider;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;
import android.view.animation.DecelerateInterpolator;
import android.widget.FrameLayout;

/**
 * The "display over other apps" panel: a foreground service owning a
 * TYPE_APPLICATION_OVERLAY window with a {@link PatchSurfaceView} that Rust
 * renders the Slint OverlayWindow into. Java owns everything about the window
 * itself: size (bubble or panel), dragging, edge snapping, focusability for
 * the keyboard, and the shadow the software renderer can't draw.
 */
public final class OverlayService extends Service implements PatchSurfaceView.FocusHost {
    static final String ACTION_HIDE = "com.mdhaarith.patch.action.HIDE_OVERLAY";

    private static final int BUBBLE_DP = 60;
    /** The Slint bubble is a 52px circle centred in its 60px window. */
    private static final int BUBBLE_INSET_DP = 4;
    private static final int PANEL_W_DP = 340;
    private static final int PANEL_H_DP = 500;
    private static final int PANEL_RADIUS_DP = 24;
    /** The panel's header (clock and buttons) doubles as its drag handle. */
    private static final int HEADER_DP = 60;
    /** Room around the surface for the elevation shadow. */
    private static final int SHADOW_DP = 12;
    private static final int ELEVATION_DP = 8;
    private static final int EDGE_GAP_DP = 4;

    private static OverlayService instance;

    private WindowManager wm;
    private Frame frame;
    private PatchSurfaceView view;
    private WindowManager.LayoutParams lp;
    private boolean expanded;
    private boolean focusable;
    private boolean imeShown;
    private int imeShift;
    private ValueAnimator snap;
    private float density;

    static void start(Context context) {
        try {
            context.startForegroundService(new Intent(context, OverlayService.class));
        } catch (RuntimeException e) {
            // Background start restrictions; the toggle stays as Rust left it.
            Log.w(PatchNative.TAG, "can't start the overlay", e);
        }
    }

    static void stop(Context context) {
        context.stopService(new Intent(context, OverlayService.class));
    }

    static void expand(boolean expanded) {
        if (instance != null) {
            instance.setExpanded(expanded);
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        PatchNative.init(this);
        instance = this;
        wm = getSystemService(WindowManager.class);
        density = getResources().getDisplayMetrics().density;
        // Show the window before going foreground: since Android 15 an app
        // may only start a foreground service from the background (boot,
        // sticky restart) while it has a visible overlay.
        if (Settings.canDrawOverlays(this)) {
            addOverlay();
        }
        try {
            if (Build.VERSION.SDK_INT >= 34) {
                startForeground(Notifications.OVERLAY_ID, Notifications.overlay(this),
                        ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
            } else {
                startForeground(Notifications.OVERLAY_ID, Notifications.overlay(this));
            }
        } catch (RuntimeException e) {
            Log.w(PatchNative.TAG, "overlay can't run in the foreground", e);
        }
        if (frame == null) {
            stopSelf();
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_HIDE.equals(intent.getAction())) {
            Prefs.set(this, Prefs.OVERLAY, false);
            PatchNative.nativeOverlayClosed();
            stopSelf();
            return START_NOT_STICKY;
        }
        // Restarted after the process was killed: come back.
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        if (snap != null) {
            snap.cancel();
        }
        if (frame != null) {
            // Immediate, so surfaceDestroyed reaches Rust before the view is
            // declared gone.
            wm.removeViewImmediate(frame);
            frame = null;
            PatchNative.nativeViewGone(PatchNative.OVERLAY);
        }
        if (instance == this) {
            instance = null;
        }
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public void onConfigurationChanged(Configuration config) {
        super.onConfigurationChanged(config);
        float d = getResources().getDisplayMetrics().density;
        if (d != density) {
            density = d;
            PatchNative.nativeDensity(d);
        }
        if (frame != null) {
            applySize();
            if (!expanded) {
                snapToEdge(false);
            }
        }
    }

    // ---- Window ---------------------------------------------------------------

    private int dp(float v) {
        return Math.round(v * density);
    }

    private void addOverlay() {
        expanded = Prefs.is(this, Prefs.OVERLAY_EXPANDED);
        view = new PatchSurfaceView(this, PatchNative.OVERLAY);
        view.setFocusHost(this);
        view.setElevation(dp(ELEVATION_DP));
        view.setOutlineProvider(new ViewOutlineProvider() {
            @Override
            public void getOutline(View v, Outline outline) {
                if (expanded) {
                    outline.setRoundRect(0, 0, v.getWidth(), v.getHeight(), dp(PANEL_RADIUS_DP));
                } else {
                    int inset = dp(BUBBLE_INSET_DP);
                    outline.setOval(inset, inset, v.getWidth() - inset, v.getHeight() - inset);
                }
            }
        });
        frame = new Frame(this);
        frame.setClipChildren(false);
        frame.setClipToPadding(false);
        frame.addView(view, new FrameLayout.LayoutParams(dp(BUBBLE_DP), dp(BUBBLE_DP)));

        int flags = WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                | WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL
                | WindowManager.LayoutParams.FLAG_WATCH_OUTSIDE_TOUCH
                | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS
                | WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED;
        lp = new WindowManager.LayoutParams(dp(BUBBLE_DP), dp(BUBBLE_DP),
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY, flags, PixelFormat.TRANSLUCENT);
        lp.gravity = Gravity.TOP | Gravity.LEFT;
        lp.setTitle("Patch");
        lp.windowAnimations = android.R.style.Animation_Toast;
        if (Build.VERSION.SDK_INT >= 30) {
            // We place and clamp the window ourselves, and move it for the
            // keyboard from the insets.
            lp.setFitInsetsTypes(0);
            lp.softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING;
        } else {
            lp.softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_PAN;
        }

        Rect screen = screen();
        SharedPreferences prefs = Prefs.get(this);
        int bubbleX = prefs.getInt(Prefs.BUBBLE_X, screen.right - dp(BUBBLE_DP + EDGE_GAP_DP));
        int bubbleY = prefs.getInt(Prefs.BUBBLE_Y, screen.top + (screen.height() - dp(BUBBLE_DP)) / 3);
        place(bubbleX, bubbleY);
        applySize();
        try {
            wm.addView(frame, lp);
        } catch (RuntimeException e) {
            Log.w(PatchNative.TAG, "can't add the overlay window", e);
            frame = null;
        }
    }

    /** Content (surface) size for the current state, fitted to the screen. */
    private int contentWidth() {
        if (!expanded) {
            return dp(BUBBLE_DP);
        }
        return Math.min(dp(PANEL_W_DP), screen().width() - 2 * dp(EDGE_GAP_DP));
    }

    private int contentHeight() {
        if (!expanded) {
            return dp(BUBBLE_DP);
        }
        return Math.min(dp(PANEL_H_DP), screen().height() - 2 * dp(EDGE_GAP_DP));
    }

    /** Positions the window so the content's top-left is at (x, y). */
    private void place(int x, int y) {
        int m = dp(SHADOW_DP);
        lp.x = x - m;
        lp.y = y - m;
    }

    private int contentX() {
        return lp.x + dp(SHADOW_DP);
    }

    private int contentY() {
        return lp.y + dp(SHADOW_DP);
    }

    /** Applies the content size and clamps the position into the screen. */
    private void applySize() {
        int w = contentWidth();
        int h = contentHeight();
        int m = dp(SHADOW_DP);
        FrameLayout.LayoutParams vlp = (FrameLayout.LayoutParams) view.getLayoutParams();
        vlp.width = w;
        vlp.height = h;
        vlp.leftMargin = m;
        vlp.topMargin = m;
        view.setLayoutParams(vlp);
        view.invalidateOutline();
        lp.width = w + 2 * m;
        lp.height = h + 2 * m;
        Rect screen = screen();
        int x = clamp(contentX(), screen.left, screen.right - w);
        int y = clamp(contentY(), screen.top, screen.bottom - h);
        place(x, y);
        update();
    }

    private void update() {
        if (frame != null && frame.isAttachedToWindow()) {
            wm.updateViewLayout(frame, lp);
        }
    }

    private static int clamp(int v, int lo, int hi) {
        return Math.max(lo, Math.min(v, Math.max(lo, hi)));
    }

    /** The usable screen: display bounds minus status bar, cutout and navigation bar. */
    private Rect screen() {
        Rect r = new Rect();
        Rect insets = new Rect();
        if (Build.VERSION.SDK_INT >= 30) {
            WindowMetrics metrics = wm.getCurrentWindowMetrics();
            r.set(metrics.getBounds());
            android.graphics.Insets i = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            insets.set(i.left, i.top, i.right, i.bottom);
        } else {
            DisplayMetrics dm = new DisplayMetrics();
            wm.getDefaultDisplay().getRealMetrics(dm);
            r.set(0, 0, dm.widthPixels, dm.heightPixels);
            insets.set(0, dp(24), 0, dp(48));
        }
        r.left += insets.left;
        r.top += insets.top;
        r.right -= insets.right;
        r.bottom -= insets.bottom;
        return r;
    }

    // ---- Bubble <-> panel -----------------------------------------------------

    private void setExpanded(boolean value) {
        if (frame == null || expanded == value) {
            return;
        }
        if (snap != null) {
            snap.cancel();
        }
        Rect screen = screen();
        boolean rightHalf = contentX() + contentWidth() / 2 > screen.centerX();
        int oldW = contentWidth();
        int oldX = contentX();
        int oldY = contentY();
        expanded = value;
        int newW = contentWidth();
        // Grow from (and shrink back to) the side of the screen the bubble
        // lives on, keeping the top edge where it was.
        int x = rightHalf ? oldX + oldW - newW : oldX;
        place(x, oldY);
        applySize();
        if (!expanded) {
            if (focusable) {
                setImeFocus(false);
            }
            snapToEdge(true);
        }
    }

    private void snapToEdge(boolean animate) {
        Rect screen = screen();
        int w = contentWidth();
        boolean right = contentX() + w / 2 > screen.centerX();
        int target = right ? screen.right - w - dp(EDGE_GAP_DP) : screen.left + dp(EDGE_GAP_DP);
        if (!animate) {
            place(target, contentY());
            update();
            saveBubble();
            return;
        }
        if (snap != null) {
            snap.cancel();
        }
        snap = ValueAnimator.ofInt(contentX(), target);
        snap.setDuration(220);
        snap.setInterpolator(new DecelerateInterpolator(1.6f));
        snap.addUpdateListener(a -> {
            place((Integer) a.getAnimatedValue(), contentY());
            update();
        });
        snap.addListener(new android.animation.AnimatorListenerAdapter() {
            @Override
            public void onAnimationEnd(android.animation.Animator animation) {
                saveBubble();
            }
        });
        snap.start();
    }

    private void saveBubble() {
        if (!expanded) {
            Prefs.get(this).edit().putInt(Prefs.BUBBLE_X, contentX()).putInt(Prefs.BUBBLE_Y, contentY()).apply();
        }
    }

    // ---- Keyboard -------------------------------------------------------------

    /** Overlay windows are unfocusable so the app below keeps the keyboard; a
     * text field in the panel needs focus for the IME. */
    @Override
    public void setImeFocus(boolean focus) {
        if (frame == null || focusable == focus) {
            return;
        }
        focusable = focus;
        if (focus) {
            lp.flags &= ~WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE;
        } else {
            lp.flags |= WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE;
            view.imeDismissed();
            if (imeShift != 0) {
                lp.y += imeShift;
                imeShift = 0;
            }
            imeShown = false;
        }
        update();
    }

    /** Lifts the panel above the keyboard (API 30+, where IME insets reach
     * overlay windows) and gives up focus once the keyboard is dismissed. */
    private WindowInsets onInsets(WindowInsets insets) {
        if (Build.VERSION.SDK_INT < 30) {
            return insets;
        }
        boolean visible = insets.isVisible(WindowInsets.Type.ime());
        if (visible) {
            imeShown = true;
            int overlap = insets.getInsets(WindowInsets.Type.ime()).bottom - dp(SHADOW_DP);
            if (overlap > 0) {
                int room = contentY() - screen().top;
                int lift = Math.min(overlap, Math.max(0, room));
                if (lift > 0) {
                    imeShift += lift;
                    lp.y -= lift;
                    update();
                }
            }
        } else if (imeShown) {
            imeShown = false;
            if (imeShift != 0) {
                lp.y += imeShift;
                imeShift = 0;
                update();
            }
            if (focusable) {
                setImeFocus(false);
            }
        }
        return insets;
    }

    // ---- Touch ----------------------------------------------------------------

    /**
     * Routes the overlay's touches: the bubble is all drag handle (a tap
     * expands it), the panel's header drags or taps, the rest goes to Slint.
     */
    private final class Frame extends FrameLayout {
        private static final int FORWARD = 0;
        private static final int PENDING = 1;
        private static final int DRAGGING = 2;

        private final int slop;
        private int mode;
        private float downRawX;
        private float downRawY;
        private float downX;
        private float downY;
        private int startX;
        private int startY;

        Frame(Context context) {
            super(context);
            slop = ViewConfiguration.get(context).getScaledTouchSlop();
            setOnApplyWindowInsetsListener((v, insets) -> onInsets(insets));
        }

        @Override
        public boolean dispatchTouchEvent(MotionEvent e) {
            float x = e.getX() - view.getLeft();
            float y = e.getY() - view.getTop();
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_OUTSIDE:
                    if (focusable) {
                        view.imeState(PatchNative.IME_HIDE, 0, "", 0, 0, -1, -1, 0, 0);
                    }
                    return false;
                case MotionEvent.ACTION_DOWN:
                    if (snap != null) {
                        snap.cancel();
                    }
                    downRawX = e.getRawX();
                    downRawY = e.getRawY();
                    downX = x;
                    downY = y;
                    startX = contentX();
                    startY = contentY();
                    mode = !expanded || (y < dp(HEADER_DP) && y >= 0) ? PENDING : FORWARD;
                    if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
                case MotionEvent.ACTION_MOVE:
                    if (mode == PENDING && Math.hypot(e.getRawX() - downRawX, e.getRawY() - downRawY) > slop) {
                        mode = DRAGGING;
                    }
                    if (mode == DRAGGING) {
                        Rect screen = screen();
                        int nx = startX + Math.round(e.getRawX() - downRawX);
                        int ny = startY + Math.round(e.getRawY() - downRawY);
                        imeShift = 0;
                        place(clamp(nx, screen.left, screen.right - contentWidth()),
                                clamp(ny, screen.top, screen.bottom - contentHeight()));
                        update();
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
                case MotionEvent.ACTION_UP:
                    if (mode == PENDING) {
                        if (expanded) {
                            view.tap(downX, downY);
                        } else {
                            // Anywhere on the bubble (or its shadow) counts.
                            view.tap(view.getWidth() / 2f, view.getHeight() / 2f);
                        }
                        performClick();
                    } else if (mode == DRAGGING) {
                        if (!expanded) {
                            snapToEdge(true);
                        }
                    } else {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    mode = FORWARD;
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    if (mode == DRAGGING && !expanded) {
                        snapToEdge(true);
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, 0, 0);
                    }
                    mode = FORWARD;
                    return true;
                default:
                    if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
            }
        }

        @Override
        public boolean performClick() {
            return super.performClick();
        }

        @Override
        public boolean dispatchKeyEvent(KeyEvent e) {
            if (e.getKeyCode() == KeyEvent.KEYCODE_BACK && focusable) {
                // Keyboard already gone (it eats the first back): release focus
                // so back reaches the app underneath again.
                if (e.getAction() == KeyEvent.ACTION_UP) {
                    setImeFocus(false);
                }
                return true;
            }
            return super.dispatchKeyEvent(e);
        }
    }
}
