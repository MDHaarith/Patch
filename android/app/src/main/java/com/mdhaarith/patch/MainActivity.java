package com.mdhaarith.patch;

import android.Manifest;
import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.drawable.ColorDrawable;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.widget.FrameLayout;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;

import java.lang.ref.WeakReference;

/** The app screen: one edge-to-edge surface Rust renders the Slint MainWindow into. */
public final class MainActivity extends Activity {
    static final String EXTRA_TASK_ID = "task_id";

    private PatchSurfaceView view;
    private OnBackInvokedCallback backCallback;
    private int sideLeft = -1;
    private int sideRight = -1;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        PatchNative.init(this);
        PatchNative.activity = new WeakReference<>(this);

        Window window = getWindow();
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false);
        } else {
            window.getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }

        view = new PatchSurfaceView(this, PatchNative.MAIN);
        setContentView(view, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        view.setOnApplyWindowInsetsListener((v, insets) -> applyInsets(insets));
        view.requestFocus();
        applyLook(PatchNative.look);

        if (Build.VERSION.SDK_INT >= 33) {
            backCallback = PatchNative::nativeBack;
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    OnBackInvokedDispatcher.PRIORITY_DEFAULT, backCallback);
            if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 1);
            }
        }
        handleIntent(getIntent());
    }

    /**
     * Status and navigation bars go to Rust as safe-area padding, so the
     * matte backdrop runs underneath them; the keyboard counts as bottom
     * inset. Side insets (landscape, cutouts) shrink the surface instead.
     */
    private WindowInsets applyInsets(WindowInsets insets) {
        int top;
        int bottom;
        int left;
        int right;
        if (Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            android.graphics.Insets ime = insets.getInsets(WindowInsets.Type.ime());
            top = bars.top;
            bottom = Math.max(bars.bottom, ime.bottom);
            left = bars.left;
            right = bars.right;
        } else {
            top = insets.getSystemWindowInsetTop();
            bottom = insets.getSystemWindowInsetBottom();
            left = insets.getSystemWindowInsetLeft();
            right = insets.getSystemWindowInsetRight();
        }
        if (left != sideLeft || right != sideRight) {
            sideLeft = left;
            sideRight = right;
            FrameLayout.LayoutParams lp = (FrameLayout.LayoutParams) view.getLayoutParams();
            lp.leftMargin = left;
            lp.rightMargin = right;
            view.setLayoutParams(lp);
        }
        PatchNative.nativeInsets(top, bottom);
        return Build.VERSION.SDK_INT >= 30 ? WindowInsets.CONSUMED : insets.consumeSystemWindowInsets();
    }

    /** Window background and system bar icons follow the app's look. */
    void applyLook(int look) {
        int background = look == 2 ? 0xFFF2F2F7 : look == 1 ? 0xFF1D555E : 0xFF0B0B0C;
        Window window = getWindow();
        window.setBackgroundDrawable(new ColorDrawable(background));
        boolean lightBars = look == 2;
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController c = window.getInsetsController();
            if (c != null) {
                int mask = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                        | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                c.setSystemBarsAppearance(lightBars ? mask : 0, mask);
            }
        } else {
            View decor = window.getDecorView();
            int flags = decor.getSystemUiVisibility();
            int light = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
            decor.setSystemUiVisibility(lightBars ? flags | light : flags & ~light);
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleIntent(intent);
    }

    private void handleIntent(Intent intent) {
        String id = intent == null ? null : intent.getStringExtra(EXTRA_TASK_ID);
        if (id != null) {
            intent.removeExtra(EXTRA_TASK_ID);
            PatchNative.nativeOpenTask(id);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        PatchNative.activity = new WeakReference<>(this);
        // The user may come back from the overlay permission screen, or have
        // revoked it: then the overlay service must not linger.
        boolean allowed = Settings.canDrawOverlays(this);
        if (!allowed && Prefs.is(this, Prefs.OVERLAY)) {
            Prefs.set(this, Prefs.OVERLAY, false);
            OverlayService.stop(this);
        }
        PatchNative.nativeOverlayAllowed(allowed);
    }

    @Override
    protected void onPause() {
        PatchNative.nativePause();
        super.onPause();
    }

    @Override
    @SuppressWarnings("deprecation")
    @SuppressLint("MissingSuperCall")
    public void onBackPressed() {
        // API < 33; newer versions use the OnBackInvokedCallback.
        PatchNative.nativeBack();
    }

    @Override
    public void onConfigurationChanged(Configuration config) {
        super.onConfigurationChanged(config);
        PatchNative.nativeDensity(getResources().getDisplayMetrics().density);
    }

    @Override
    protected void onDestroy() {
        if (Build.VERSION.SDK_INT >= 33 && backCallback != null) {
            getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(backCallback);
        }
        if (PatchNative.activity.get() == this) {
            PatchNative.activity = new WeakReference<>(null);
        }
        PatchNative.nativeViewGone(PatchNative.MAIN);
        super.onDestroy();
    }
}
