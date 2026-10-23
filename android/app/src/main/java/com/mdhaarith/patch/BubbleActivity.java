package com.mdhaarith.patch;

import android.app.Activity;
import android.content.res.Configuration;
import android.os.Bundle;
import android.widget.FrameLayout;

/** The expanded {@link Bubble}: Rust renders the Slint overlay panel into it. */
public final class BubbleActivity extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        PatchNative.init(this);
        // Before the surface arrives, so Rust lays it out as the panel.
        PatchNative.nativeBubbleOpened();
        PatchSurfaceView view = new PatchSurfaceView(this, PatchNative.OVERLAY);
        setContentView(view, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        view.requestFocus();
    }

    @Override
    protected void onPause() {
        PatchNative.nativePause();
        super.onPause();
    }

    @Override
    public void onConfigurationChanged(Configuration config) {
        super.onConfigurationChanged(config);
        PatchNative.nativeDensity(getResources().getDisplayMetrics().density);
    }

    @Override
    protected void onDestroy() {
        PatchNative.nativeViewGone(PatchNative.OVERLAY);
        super.onDestroy();
    }
}
