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
import android.view.Choreographer;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.VelocityTracker;
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
 * itself: size, dragging, placement, focusability for the keyboard, and
 * the shadow the software renderer can't draw. Collapsed it is the draggable
 * pet; expanded it fills the screen (dimmed behind) and Slint pops the panel
 * out under the pet, like a system bubble.
 */
public final class OverlayService extends Service implements PatchSurfaceView.FocusHost {
    static final String ACTION_HIDE = "com.mdhaarith.patch.action.HIDE_OVERLAY";

    private static final int BUBBLE_DP = 60;
    /** The Slint bubble is a 52px circle centred in its 60px window. */
    private static final int BUBBLE_INSET_DP = 4;
    /** Room around the surface for the elevation shadow. */
    private static final int SHADOW_DP = 12;
    private static final int ELEVATION_DP = 8;
    private static final int EDGE_GAP_DP = 4;
    private static final int TRANSITION_MS = 240;

    private static OverlayService instance;

    private WindowManager wm;
    private Frame frame;
    private PatchSurfaceView view;
    private WindowManager.LayoutParams lp;
    private boolean expanded;
    private boolean focusable;
    private boolean imeShown;
    /** Screen y of the keyboard's top edge while it shows, else 0. */
    private int imeTop;
    private ValueAnimator settle;
    private int bubbleX;
    private int bubbleY;
    private boolean targetExpanded;
    private boolean transitioning;
    private final Runnable finishTransition = () -> {
        // Release the IME while still full-size so its resize keeps the pet's saved origin.
        if (!targetExpanded && focusable) {
            setImeFocus(false);
        }
        expanded = targetExpanded;
        if (!expanded) {
            place(bubbleX, bubbleY);
            applySize();
        }
        transitioning = false;
    };
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
        if (frame != null) {
            frame.cancelPlay();
            frame.removeCallbacks(finishTransition);
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
            frame.cancelPlay();
            clampBubble();
            if (!expanded) {
                place(bubbleX, bubbleY);
            }
            applySize();
            saveBubble();
        }
    }

    // ---- Window ---------------------------------------------------------------

    private int dp(float v) {
        return Math.round(v * density);
    }

    private void addOverlay() {
        expanded = Prefs.is(this, Prefs.OVERLAY_EXPANDED);
        targetExpanded = expanded;
        view = new PatchSurfaceView(this, PatchNative.OVERLAY);
        view.setFocusHost(this);
        view.routedTouches = true;
        view.setElevation(dp(ELEVATION_DP));
        view.setOutlineProvider(new ViewOutlineProvider() {
            @Override
            public void getOutline(View v, Outline outline) {
                if (expanded) {
                    outline.setEmpty(); // The dim behind sets the panel apart.
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
        lp.windowAnimations = 0;
        disableMoveAnimation(lp);
        if (Build.VERSION.SDK_INT >= 30) {
            // We place and clamp the window ourselves, and move it for the
            // keyboard from the insets.
            lp.setFitInsetsTypes(0);
            lp.softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING;
        } else {
            lp.softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_PAN;
        }

        placeBubble();
        applySize();
        try {
            wm.addView(frame, lp);
        } catch (RuntimeException e) {
            Log.w(PatchNative.TAG, "can't add the overlay window", e);
            frame = null;
        }
    }

    /** The system slides a moved window from its old spot, so expanding drags the panel in
     * from the pet's corner and collapsing flings the pet across the screen. */
    private static void disableMoveAnimation(WindowManager.LayoutParams lp) {
        if (Build.VERSION.SDK_INT >= 34) {
            lp.setCanPlayMoveAnimation(false);
            return;
        }
        try {
            java.lang.reflect.Field f = WindowManager.LayoutParams.class.getField("privateFlags");
            f.setInt(lp, f.getInt(lp) | 0x40); // PRIVATE_FLAG_NO_MOVE_ANIMATION
        } catch (ReflectiveOperationException | RuntimeException e) {
            Log.w(PatchNative.TAG, "can't disable the overlay move animation", e);
        }
    }

    /** Restore and clamp the pet before the first surface is created. */
    private void placeBubble() {
        Rect screen = screen();
        SharedPreferences prefs = Prefs.get(this);
        bubbleX = prefs.getInt(Prefs.BUBBLE_X, screen.right - dp(BUBBLE_DP + EDGE_GAP_DP));
        bubbleY = prefs.getInt(Prefs.BUBBLE_Y, screen.top + (screen.height() - dp(BUBBLE_DP)) / 3);
        clampBubble();
        place(bubbleX, bubbleY);
        saveBubble();
    }

    /** Content (surface) size: the bubble, or the whole screen above the keyboard. */
    private int contentWidth() {
        return expanded ? screen().width() : dp(BUBBLE_DP);
    }

    private int contentHeight() {
        if (!expanded) {
            return dp(BUBBLE_DP);
        }
        Rect screen = screen();
        return Math.max(dp(BUBBLE_DP),
                (imeTop > 0 ? Math.min(screen.bottom, imeTop) : screen.bottom) - screen.top);
    }

    /** Positions the window so the content's top-left is at (x, y). */
    private void place(int x, int y) {
        int m = dp(SHADOW_DP);
        lp.x = x - m;
        lp.y = y - m;
        if (!expanded) {
            bubbleX = x;
            bubbleY = y;
        }
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
        view.setImportantForAccessibility(expanded ? View.IMPORTANT_FOR_ACCESSIBILITY_AUTO
                : View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        frame.setClickable(!expanded);
        frame.setContentDescription(expanded ? null : "Patch pet. Open tasks");
        PatchNative.nativeOverlayPosition((bubbleX - screen().left) / density,
                (bubbleY - screen().top) / density);
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
        if (frame == null || targetExpanded == value) {
            return;
        }
        frame.cancelPlay();
        clampBubble();
        targetExpanded = value;
        transitioning = true;
        frame.removeCallbacks(finishTransition);
        if (value) {
            saveBubble();
            expanded = true;
            applySize();
        } else {
            view.imeState(PatchNative.IME_HIDE, 0, "", 0, 0, -1, -1, 0, 0);
        }
        // Slint needs the full surface until its panel and backdrop have faded out.
        frame.postDelayed(finishTransition, TRANSITION_MS);
    }

    private void clampBubble() {
        Rect screen = screen();
        bubbleX = clamp(bubbleX, screen.left, screen.right - dp(BUBBLE_DP));
        bubbleY = clamp(bubbleY, screen.top, screen.bottom - dp(BUBBLE_DP));
    }

    private void settleBubble() {
        if (settle != null) {
            settle.cancel();
        }
        int x = contentX();
        int y = contentY();
        Rect bounds = screen();
        int targetX = clamp(x, bounds.left, bounds.right - dp(BUBBLE_DP));
        int targetY = clamp(y, bounds.top, bounds.bottom - dp(BUBBLE_DP));
        if (x == targetX && y == targetY) {
            saveBubble();
            return;
        }
        settle = ValueAnimator.ofFloat(0f, 1f);
        settle.setDuration(180);
        settle.setInterpolator(new DecelerateInterpolator());
        settle.addUpdateListener(a -> {
            float t = (Float) a.getAnimatedValue();
            place(x + Math.round((targetX - x) * t), y + Math.round((targetY - y) * t));
            update();
        });
        settle.addListener(new android.animation.AnimatorListenerAdapter() {
            @Override
            public void onAnimationEnd(android.animation.Animator animation) {
                saveBubble();
            }
        });
        settle.start();
    }

    private void saveBubble() {
        Prefs.get(this).edit().putInt(Prefs.BUBBLE_X, bubbleX).putInt(Prefs.BUBBLE_Y, bubbleY).apply();
    }

    // ---- Keyboard -------------------------------------------------------------

    /** Overlay windows are unfocusable so the app below keeps the keyboard; a
     * text field in the panel needs focus for the IME. */
    @Override
    public boolean setImeFocus(boolean focus) {
        if (frame == null || (focus && (!expanded || !targetExpanded))) {
            return false;
        }
        if (focus && focusable) {
            return true;
        }
        focusable = focus;
        if (focus) {
            lp.flags &= ~WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE;
        } else {
            lp.flags |= WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE;
            view.imeDismissed();
            imeShown = false;
            if (imeTop != 0) {
                imeTop = 0;
                applySize();
            }
        }
        update();
        return true;
    }

    /** Shrinks the panel's window above the keyboard (API 30+, where IME
     * insets reach overlay windows) and gives up focus once it is dismissed. */
    private WindowInsets onInsets(WindowInsets insets) {
        if (Build.VERSION.SDK_INT < 30) {
            return insets;
        }
        boolean visible = insets.isVisible(WindowInsets.Type.ime());
        if (visible) {
            imeShown = true;
            // Stays the same once the window ends at the keyboard, so no loop.
            int top = lp.y + lp.height - insets.getInsets(WindowInsets.Type.ime()).bottom;
            if (expanded && targetExpanded && top != imeTop) {
                imeTop = top;
                applySize();
            }
        } else if (imeShown) {
            imeShown = false;
            if (imeTop != 0) {
                imeTop = 0;
                applySize();
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
     * expands it); expanded, everything goes to Slint.
     */
    private final class Frame extends FrameLayout implements Choreographer.FrameCallback {
        private static final int FORWARD = 0;
        private static final int PENDING = 1;
        private static final int DRAGGING = 2;
        private static final int CANCELLED = 3;

        private final int slop;
        private final int minFling;
        private final int maxFling;
        private VelocityTracker velocity;
        private boolean flying;
        private float flightX;
        private float flightY;
        private float vx;
        private float vy;
        private long lastFrame;
        private float flightSeconds;
        private int bounces;
        private boolean hardBump;
        private int playState;
        private int mode = CANCELLED;
        private float downRawX;
        private float downRawY;
        private int startX;
        private int startY;

        Frame(Context context) {
            super(context);
            ViewConfiguration config = ViewConfiguration.get(context);
            slop = config.getScaledTouchSlop();
            minFling = config.getScaledMinimumFlingVelocity();
            maxFling = config.getScaledMaximumFlingVelocity();
            setOnApplyWindowInsetsListener((v, insets) -> onInsets(insets));
        }

        private void play(int state) {
            if (state != playState || Math.abs(state) == 3) {
                playState = state;
                PatchNative.nativePetPlay(state);
            }
        }

        private void recycleVelocity() {
            if (velocity != null) {
                velocity.recycle();
                velocity = null;
            }
        }

        private void stopMotion() {
            flying = false;
            Choreographer.getInstance().removeFrameCallback(this);
            if (settle != null) {
                settle.cancel();
                settle = null;
            }
        }

        private void cancelPlay() {
            stopMotion();
            recycleVelocity();
            if (mode == FORWARD) {
                PatchNative.nativeTouch(PatchNative.OVERLAY, PatchNative.TOUCH_CANCEL, 0, 0);
            }
            mode = CANCELLED;
            play(0);
            clampBubble();
            saveBubble();
        }

        private void track(MotionEvent e) {
            // Local coordinates move with the window; velocity must stay in screen space.
            MotionEvent raw = MotionEvent.obtain(e);
            raw.offsetLocation(e.getRawX() - e.getX(), e.getRawY() - e.getY());
            velocity.addMovement(raw);
            raw.recycle();
        }

        private void drop(MotionEvent e) {
            velocity.computeCurrentVelocity(1000, Math.min(maxFling, dp(2600)));
            vx = velocity.getXVelocity(e.getPointerId(0));
            vy = velocity.getYVelocity(e.getPointerId(0));
            if (Math.hypot(vx, vy) < minFling) {
                play(4);
                settleBubble();
                return;
            }
            clampBubble();
            place(bubbleX, bubbleY);
            update();
            flightX = bubbleX;
            flightY = bubbleY;
            flying = true;
            lastFrame = 0;
            flightSeconds = 0;
            bounces = 0;
            hardBump = false;
            play(vx < 0 ? -2 : 2);
            Choreographer.getInstance().postFrameCallback(this);
        }

        @Override
        public void doFrame(long time) {
            if (!flying) {
                return;
            }
            if (lastFrame == 0) {
                lastFrame = time;
            }
            float dt = Math.min((time - lastFrame) / 1_000_000_000f, 0.032f);
            lastFrame = time;
            flightSeconds += dt;
            float decay = (float) Math.exp(-2.2f * dt);
            flightX += vx * (1 - decay) / 2.2f;
            flightY += vy * (1 - decay) / 2.2f;
            vx *= decay;
            vy *= decay;
            Rect bounds = screen();
            int right = Math.max(bounds.left, bounds.right - dp(BUBBLE_DP));
            int bottom = Math.max(bounds.top, bounds.bottom - dp(BUBBLE_DP));
            boolean bump = false;
            float impact = (float) Math.hypot(vx, vy);
            if (flightX < bounds.left || flightX > right) {
                flightX = clamp(Math.round(flightX), bounds.left, right);
                vx = flightX <= bounds.left ? Math.abs(vx) * 0.7f : -Math.abs(vx) * 0.7f;
                if (right == bounds.left) vx = 0;
                bump = true;
            }
            if (flightY < bounds.top || flightY > bottom) {
                flightY = clamp(Math.round(flightY), bounds.top, bottom);
                vy = flightY <= bounds.top ? Math.abs(vy) * 0.7f : -Math.abs(vy) * 0.7f;
                if (bottom == bounds.top) vy = 0;
                bump = true;
            }
            place(Math.round(flightX), Math.round(flightY));
            update();
            if (bump) {
                bounces++;
                hardBump |= impact > dp(1200);
                play(vx < 0 ? -3 : 3);
            }
            if (Math.hypot(vx, vy) < dp(35) || flightSeconds >= 4) {
                flying = false;
                play(hardBump || bounces >= 3 ? 5 : 4);
                saveBubble();
            } else {
                Choreographer.getInstance().postFrameCallback(this);
            }
        }

        @Override
        public boolean dispatchTouchEvent(MotionEvent e) {
            if (transitioning) {
                return true;
            }
            if (velocity != null && e.getActionMasked() != MotionEvent.ACTION_DOWN) {
                track(e);
            }
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_OUTSIDE:
                    // Keyboard taps land outside the window too; only bar taps close.
                    if (expanded && !imeShown && !focusable) {
                        view.tap(0, 0);
                    }
                    return true;
                case MotionEvent.ACTION_DOWN:
                    boolean caught = flying;
                    stopMotion();
                    recycleVelocity();
                    downRawX = e.getRawX();
                    downRawY = e.getRawY();
                    startX = contentX();
                    startY = contentY();
                    mode = expanded ? FORWARD : caught ? DRAGGING : PENDING;
                    if (mode != FORWARD) {
                        velocity = VelocityTracker.obtain();
                        track(e);
                        play(caught ? 1 : 0);
                    } else {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
                case MotionEvent.ACTION_POINTER_DOWN:
                    if (mode == PENDING || mode == DRAGGING) {
                        if (mode == DRAGGING) {
                            play(4);
                            settleBubble();
                        }
                        recycleVelocity();
                        mode = CANCELLED;
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
                case MotionEvent.ACTION_MOVE:
                    if (mode == PENDING && Math.hypot(e.getRawX() - downRawX, e.getRawY() - downRawY) > slop) {
                        mode = DRAGGING;
                        play(e.getRawX() < downRawX ? -1 : 1);
                    }
                    if (mode == DRAGGING) {
                        Rect screen = screen();
                        int nx = startX + Math.round(e.getRawX() - downRawX);
                        int ny = startY + Math.round(e.getRawY() - downRawY);
                        if (nx != contentX()) {
                            play(nx < contentX() ? -1 : 1);
                        }
                        int over = dp(16);
                        place(clamp(nx, screen.left - over, screen.right - contentWidth() + over),
                                clamp(ny, screen.top - over, screen.bottom - contentHeight() + over));
                        update();
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    return true;
                case MotionEvent.ACTION_UP:
                    if (mode == PENDING) {
                        if (e.getEventTime() - e.getDownTime() < ViewConfiguration.getLongPressTimeout()
                                && Math.hypot(e.getRawX() - downRawX, e.getRawY() - downRawY) <= slop) {
                            performClick();
                        }
                    } else if (mode == DRAGGING) {
                        drop(e);
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    recycleVelocity();
                    mode = CANCELLED;
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    if (mode == DRAGGING) {
                        play(4);
                        settleBubble();
                    } else if (mode == FORWARD) {
                        view.forwardTouch(e, -view.getLeft(), -view.getTop());
                    }
                    recycleVelocity();
                    mode = CANCELLED;
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
            super.performClick();
            if (!expanded && !transitioning) {
                view.tap(view.getWidth() / 2f, view.getHeight() / 2f);
            }
            return true;
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
