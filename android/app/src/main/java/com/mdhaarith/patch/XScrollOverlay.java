package com.mdhaarith.patch;

import android.accessibilityservice.AccessibilityService;
import android.animation.ArgbEvaluator;
import android.animation.ValueAnimator;
import android.app.Dialog;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.RadialGradient;
import android.graphics.Rect;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.view.animation.DecelerateInterpolator;
import android.view.animation.OvershootInterpolator;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.util.Locale;

/**
 * Accessibility overlays drawn over video apps: a blurred circle with the video count under
 * the camera, and a full-screen block screen with Patch's worried pet.
 * TYPE_ACCESSIBILITY_OVERLAY needs no "Display over other apps" permission.
 */
final class XScrollOverlay {
    private static final int SIZE_DP = 36;

    private final AccessibilityService service;
    private final WindowManager wm;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final float density;

    // A Dialog because only a Window can blur what is behind it (setBackgroundBlurRadius, API 31).
    private final Dialog circle;
    private final Counter count;
    private final FrameLayout block;
    private final TextView blockBody;
    private final TextView blockCountdown;
    private int secondsLeft;

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (secondsLeft <= 0) {
                closeX();
                return;
            }
            blockCountdown.setText("Closing in " + secondsLeft + "…");
            secondsLeft--;
            handler.postDelayed(this, 1000);
        }
    };

    XScrollOverlay(AccessibilityService service) {
        this.service = service;
        wm = service.getSystemService(WindowManager.class);
        density = service.getResources().getDisplayMetrics().density;

        count = new Counter(service);
        GradientDrawable bg = new GradientDrawable();
        bg.setShape(GradientDrawable.OVAL); // its outline also rounds the blur
        circle = new Dialog(service, android.R.style.Theme_Translucent_NoTitleBar);
        circle.setContentView(count);
        Window w = circle.getWindow();
        w.setType(WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY);
        w.setFormat(PixelFormat.TRANSLUCENT);
        w.setBackgroundDrawable(bg);
        w.setWindowAnimations(android.R.style.Animation_Toast); // fade in and out
        w.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        w.addFlags(WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE | WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
                | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS);
        WindowManager.LayoutParams lp = w.getAttributes();
        // Screen coordinates, so the circle sits just under the camera hole instead of below the status bar.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P)
            lp.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        lp.gravity = Gravity.TOP | Gravity.CENTER_HORIZONTAL;
        lp.width = lp.height = dp(SIZE_DP);
        int screenW = service.getResources().getDisplayMetrics().widthPixels;
        Rect cam = camera(screenW);
        lp.x = cam.centerX() - screenW / 2;
        lp.y = cam.bottom + dp(12);
        w.setAttributes(lp);
        bg.setColor(0xB3000000); // no blur: a darker fill keeps the number readable over video
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            w.setBackgroundBlurRadius(dp(20)); // the OS (OriginOS) blurs the app behind, live
            // Blur turns off in battery saver and on devices without it; Android tells us either way.
            wm.addCrossWindowBlurEnabledListener(service.getMainExecutor(), on -> bg.setColor(on ? 0x40000000 : 0xB3000000));
        }

        blockBody = text("", 16, 0xB3FFFFFF, false);
        blockCountdown = text("", 14, 0x80FFFFFF, false);
        blockCountdown.setPadding(0, dp(32), 0, dp(16));

        LinearLayout col = new LinearLayout(service);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setGravity(Gravity.CENTER_HORIZONTAL);
        col.setPadding(dp(32), 0, dp(32), 0);
        ImageView pet = new ImageView(service);
        Drawable worried = service.getDrawable(R.drawable.pet_worried);
        if (worried != null) worried.setFilterBitmap(false); // keep the pixel art crisp
        pet.setImageDrawable(worried);
        col.addView(pet, new LinearLayout.LayoutParams(dp(112), dp(112)));
        TextView title = text("Enough videos for today", 28, Color.WHITE, true);
        title.setPadding(0, dp(24), 0, dp(12));
        col.addView(title);
        col.addView(blockBody);
        col.addView(blockCountdown);
        TextView close = text("Close app now", 16, Color.BLACK, true);
        GradientDrawable closeBg = new GradientDrawable();
        closeBg.setColor(Color.WHITE);
        closeBg.setCornerRadius(dp(28));
        close.setBackground(closeBg);
        close.setPadding(dp(32), dp(14), dp(32), dp(14));
        close.setOnClickListener(v -> closeX());
        col.addView(close, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));

        block = new FrameLayout(service);
        block.setBackgroundColor(0xF5000000);
        block.setClickable(true); // swallow touches so X underneath can't be scrolled
        block.addView(col, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.CENTER));
    }

    /** The top camera cutout, or a status-bar-high strip at the top centre when there is none. */
    private Rect camera(int screenW) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            DisplayCutout cut = wm.getCurrentWindowMetrics().getWindowInsets().getDisplayCutout();
            if (cut != null && !cut.getBoundingRectTop().isEmpty()) return cut.getBoundingRectTop();
        }
        int id = service.getResources().getIdentifier("status_bar_height", "dimen", "android");
        int h = id > 0 ? service.getResources().getDimensionPixelSize(id) : dp(24);
        return new Rect(screenW / 2, 0, screenW / 2, h);
    }

    /** Shows today's video count while a video app is open. today = {scrolls, posts, videos, ms}. */
    void showCounter(long[] today, int limit) {
        if (block.isAttachedToWindow()) return;
        boolean appearing = !circle.isShowing();
        count.set(today[2], limit > 0 ? (float) today[2] / limit : -1f, appearing);
        if (appearing) circle.show();
    }

    /** The video app was left. */
    void hide() {
        count.stop();
        if (circle.isShowing()) circle.hide();
    }

    /** Covers X, counts down, then sends the user home. today = {scrolls, posts, videos, ms}. */
    void block(long[] today, int limit) {
        if (block.isAttachedToWindow()) return;
        hide();
        blockBody.setText(String.format(Locale.US,
                "You hit your limit of %d videos.\n%d min in video apps today.\nThey stay closed until midnight.",
                limit, today[3] / 60_000));
        WindowManager.LayoutParams lp = new WindowManager.LayoutParams(
                WindowManager.LayoutParams.MATCH_PARENT, WindowManager.LayoutParams.MATCH_PARENT,
                WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
                PixelFormat.TRANSLUCENT);
        block.setAlpha(0f);
        wm.addView(block, lp);
        block.animate().alpha(1f).setDuration(250);
        secondsLeft = 5;
        tick.run();
    }

    private void closeX() {
        handler.removeCallbacks(tick);
        service.performGlobalAction(AccessibilityService.GLOBAL_ACTION_HOME);
        // Remove after home is shown so X never flashes back; the countdown also guarantees removal.
        handler.postDelayed(() -> {
            if (block.isAttachedToWindow()) wm.removeView(block);
        }, 300);
    }

    private TextView text(String s, float size, int color, boolean bold) {
        TextView t = new TextView(service);
        t.setText(s);
        t.setTextSize(size);
        t.setTextColor(color);
        t.setGravity(Gravity.CENTER);
        t.setTypeface(bold ? Typeface.DEFAULT_BOLD : Typeface.DEFAULT);
        t.setLineSpacing(0f, 1.25f);
        return t;
    }

    private int dp(int v) {
        return Math.round(v * density);
    }

    /** Colour for progress toward the limit (iOS system colours); < 0 = no limit. */
    static int levelColor(float progress) {
        if (progress < 0f) return Color.WHITE;
        if (progress < 0.5f) return 0xFF30D158; // green
        if (progress < 0.75f) return 0xFFFFD60A; // yellow
        if (progress < 0.9f) return 0xFFFF9F0A; // orange
        return 0xFFFF453A; // red
    }

    /**
     * Drawn on the blurred circle: a glass rim and top sheen, a glowing progress ring that sweeps
     * and shifts colour as videos add up, a pop on each new video, and a breathing glow near the limit.
     */
    private static final class Counter extends View {
        private static final ArgbEvaluator ARGB = new ArgbEvaluator();

        private final float d;
        private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint label = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint sheen = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF box = new RectF();

        private String text = "0";
        private long shownCount = -1;
        private boolean limited; // a limit is set, so the ring shows progress
        private float sweep; // drawn progress, 0..1, animated toward the real one
        private int color = Color.WHITE; // drawn ring colour, animated with sweep
        private float pop = 1f; // content scale for the new-video pop
        private float breath; // 0..1 glow pulse at >= 90%

        private ValueAnimator fill, bounce;
        private final ValueAnimator pulse = ValueAnimator.ofFloat(0f, 1f);

        Counter(Context context) {
            super(context);
            d = context.getResources().getDisplayMetrics().density;
            ring.setStyle(Paint.Style.STROKE);
            ring.setStrokeWidth(3 * d);
            ring.setStrokeCap(Paint.Cap.ROUND);
            label.setColor(Color.WHITE);
            label.setTypeface(Typeface.DEFAULT_BOLD);
            label.setTextAlign(Paint.Align.CENTER);
            label.setShadowLayer(1.5f * d, 0, 0.5f * d, 0x66000000); // lifts the number off bright video
            rim.setStyle(Paint.Style.STROKE);
            rim.setStrokeWidth(d);
            pulse.setDuration(1100);
            pulse.setRepeatMode(ValueAnimator.REVERSE);
            pulse.setRepeatCount(ValueAnimator.INFINITE);
            pulse.addUpdateListener(a -> {
                breath = (float) a.getAnimatedValue();
                invalidate();
            });
        }

        /** progress < 0 = no limit. appearing = the circle is about to show, so the ring sweeps in from empty. */
        void set(long videos, float progress, boolean appearing) {
            boolean more = shownCount >= 0 && videos > shownCount && !appearing;
            shownCount = videos;
            text = String.valueOf(videos);
            limited = progress >= 0f;
            float target = limited ? Math.min(1f, progress) : 0f;
            int targetColor = levelColor(progress);
            if (appearing) {
                sweep = 0f;
                color = limited ? levelColor(0f) : targetColor;
            }
            if (fill != null) fill.cancel();
            float fromSweep = sweep;
            int fromColor = color;
            fill = ValueAnimator.ofFloat(0f, 1f).setDuration(appearing ? 900 : 600);
            fill.setInterpolator(new DecelerateInterpolator(2f));
            fill.addUpdateListener(a -> {
                float t = (float) a.getAnimatedValue();
                sweep = fromSweep + (target - fromSweep) * t;
                color = (int) ARGB.evaluate(t, fromColor, targetColor);
                invalidate();
            });
            fill.start();
            if (more) {
                if (bounce != null) bounce.cancel();
                // Shrinks, then overshoots back; the ring sits 4dp in, so the overshoot never clips.
                bounce = ValueAnimator.ofFloat(0.8f, 1f).setDuration(450);
                bounce.setInterpolator(new OvershootInterpolator(3f));
                bounce.addUpdateListener(a -> {
                    pop = (float) a.getAnimatedValue();
                    invalidate();
                });
                bounce.start();
            }
            boolean near = limited && progress >= 0.9f;
            if (near && !pulse.isStarted()) pulse.start();
            else if (!near && pulse.isStarted()) {
                pulse.cancel();
                breath = 0f;
            }
            invalidate();
        }

        /** The circle hid: stop the animations so nothing runs off screen. */
        void stop() {
            if (fill != null) fill.end();
            if (bounce != null) bounce.end();
            pulse.cancel();
            breath = 0f;
        }

        @Override
        protected void onSizeChanged(int w, int h, int oldw, int oldh) {
            // Light from the top left, like the OS glass panels: bright edge there, faint at the bottom right.
            rim.setShader(new LinearGradient(0, 0, w, h,
                    new int[] {0x99FFFFFF, 0x0DFFFFFF, 0x33FFFFFF}, new float[] {0f, 0.55f, 1f}, Shader.TileMode.CLAMP));
            sheen.setShader(new RadialGradient(w / 2f, h * 0.1f, w * 0.55f,
                    new int[] {0x2EFFFFFF, 0x00FFFFFF}, null, Shader.TileMode.CLAMP));
        }

        @Override
        protected void onDraw(Canvas c) {
            float r = getWidth() / 2f;
            c.drawCircle(r, r, r, sheen);
            c.drawCircle(r, r, r - d / 2, rim);

            c.save();
            c.scale(pop, pop, r, r);
            float rr = r - 4 * d;
            box.set(r - rr, r - rr, r + rr, r + rr);
            ring.clearShadowLayer();
            ring.setColor(0x2EFFFFFF);
            c.drawOval(box, ring);
            if (limited && sweep > 0f) {
                ring.setColor(color);
                // Glow in the ring's colour; shadows on strokes need hardware support from Android 9.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P)
                    ring.setShadowLayer((2.5f + 2f * breath) * d, 0, 0,
                            (color & 0x00FFFFFF) | ((int) (0x99 + 0x66 * breath) << 24));
                c.drawArc(box, -90f, 360f * sweep, false, ring);
            }
            label.setTextSize((text.length() <= 2 ? 13 : text.length() == 3 ? 10 : 8) * d); // fit inside the ring
            c.drawText(text, r, r - (label.ascent() + label.descent()) / 2, label);
            c.restore();
        }
    }
}
