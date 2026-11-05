package com.mdhaarith.patch;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/** Takes taps on widget rows. Invisible, so ticking a task doesn't open the app. */
public final class WidgetRowActivity extends Activity {
    static final String EXTRA_COMPLETE = "complete";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent intent = getIntent();
        String id = intent.getStringExtra(MainActivity.EXTRA_TASK_ID);
        if (id != null && intent.getBooleanExtra(EXTRA_COMPLETE, false)) {
            PatchNative.init(getApplicationContext());
            // Waits on the Rust thread, which may be starting up.
            new Thread(() -> PatchNative.nativeCompleteTask(id), "patch-widget-done").start();
        } else {
            Intent open = new Intent(this, MainActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            if (id != null) {
                open.putExtra(MainActivity.EXTRA_TASK_ID, id);
            }
            startActivity(open);
        }
        finish();
    }
}
