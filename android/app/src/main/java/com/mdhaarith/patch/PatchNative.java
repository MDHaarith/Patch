package com.mdhaarith.patch;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.net.ConnectivityManager;
import android.net.Network;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;
import android.provider.Settings;
import android.util.Log;
import android.view.Surface;
import android.widget.Toast;

import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning;

import java.lang.ref.WeakReference;
import java.util.ArrayDeque;

/**
 * The only JNI boundary. {@code native*} methods go to Rust
 * ({@code crates/android/src/android/mod.rs}); the other static methods are
 * called by Rust's UI thread ({@code bridge.rs}) and must return promptly:
 * anything touching views is posted to the main thread.
 *
 * <p>Names and signatures on both sides are checked by
 * {@code crates/android/tests/jni_contract.rs}.
 */
final class PatchNative {
    static final String TAG = "Patch";

    static final int MAIN = 0;
    static final int OVERLAY = 1;

    static final int TOUCH_DOWN = 0;
    static final int TOUCH_MOVE = 1;
    static final int TOUCH_UP = 2;
    static final int TOUCH_CANCEL = 3;
    static final int TOUCH_LONG_PRESS = 4;

    static final int IME_COMMIT = 0;
    static final int IME_COMPOSE = 1;
    static final int IME_FINISH = 2;
    static final int IME_DELETE = 3;
    static final int IME_SELECT = 4;
    static final int IME_REGION = 5;
    static final int IME_ACTION = 6;

    static final int IME_HIDE = 0;
    static final int IME_UPDATE = 1;
    static final int IME_SHOW = 2;
    static final int IME_SHOW_ACTIONS = 3;

    /** Look of the app: 0 matte black, 1 black glass, 2 light, 3 light glass. */
    static volatile int look = 0;
    static WeakReference<MainActivity> activity = new WeakReference<>(null);

    private static Context app;
    private static Handler main;
    private static ClipboardManager clipboard;
    private static ConnectivityManager.NetworkCallback networkCallback;

    private static void watchNetwork() {
        if (networkCallback != null) return;
        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) {
                nativeSyncNow();
            }
        };
        app.getSystemService(ConnectivityManager.class).registerDefaultNetworkCallback(networkCallback);
    }

    private static void unwatchNetwork() {
        if (networkCallback == null) return;
        app.getSystemService(ConnectivityManager.class).unregisterNetworkCallback(networkCallback);
        networkCallback = null;
    }
    private static final PatchSurfaceView[] views = new PatchSurfaceView[2];
    @SuppressWarnings("unchecked")
    private static final ArrayDeque<Runnable>[] frameWaiters = new ArrayDeque[] {new ArrayDeque<>(), new ArrayDeque<>()};

    private PatchNative() {}

    /** Loads the library and starts the Rust UI thread once. Main thread. */
    static void init(Context context) {
        if (app == null) {
            app = context.getApplicationContext();
            main = new Handler(Looper.getMainLooper());
            clipboard = app.getSystemService(ClipboardManager.class);
            System.loadLibrary("patch_android");
            Notifications.createChannels(app);
        }
        nativeInit(app, app.getFilesDir().getAbsolutePath(), app.getResources().getDisplayMetrics().density);
    }

    static void register(int kind, PatchSurfaceView view) {
        views[kind] = view;
        watchNetwork();
    }

    static void unregister(int kind, PatchSurfaceView view) {
        if (views[kind] == view) {
            views[kind] = null;
            if (views[MAIN] == null && views[OVERLAY] == null) unwatchNetwork();
        }
    }

    /**
     * Holds {@code drawingFinished} (from surfaceRedrawNeededAsync) until Rust
     * has posted a frame at the new size, so the window manager never shows a
     * stale or stretched frame. A timeout keeps a stuck frame from freezing the
     * window.
     */
    static void awaitFrame(int kind, Runnable drawingFinished) {
        Runnable once = new Runnable() {
            private boolean done;

            @Override
            public void run() {
                if (!done) {
                    done = true;
                    frameWaiters[kind].remove(this);
                    drawingFinished.run();
                }
            }
        };
        frameWaiters[kind].add(once);
        main.postDelayed(once, 1000);
        nativeRedrawNeeded(kind);
    }

    static void openOverlaySettings(Context context) {
        Intent intent = new Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:" + context.getPackageName()));
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            context.startActivity(intent);
        } catch (RuntimeException e) {
            Log.w(TAG, "no overlay permission screen", e);
        }
    }

    // ---- Called from Rust's UI thread ------------------------------------

    static boolean setOverlay(boolean on) {
        if (on && !Settings.canDrawOverlays(app)) {
            main.post(() -> openOverlaySettings(app));
            return false;
        }
        Prefs.set(app, Prefs.OVERLAY, on);
        main.post(() -> {
            if (on) {
                OverlayService.start(app);
            } else {
                OverlayService.stop(app);
            }
        });
        return true;
    }

    static boolean overlayAllowed() {
        return Settings.canDrawOverlays(app);
    }

    static void setOverlayBoot(boolean on) {
        Prefs.set(app, Prefs.OVERLAY_BOOT, on);
    }

    static void overlayExpand(boolean expanded) {
        Prefs.set(app, Prefs.OVERLAY_EXPANDED, expanded);
        main.post(() -> OverlayService.expand(expanded));
    }

    static void openApp(String taskId) {
        main.post(() -> {
            Intent intent = new Intent(app, MainActivity.class);
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            if (taskId != null) {
                intent.putExtra(MainActivity.EXTRA_TASK_ID, taskId);
            }
            try {
                app.startActivity(intent);
            } catch (RuntimeException e) {
                Log.w(TAG, "can't open the app", e);
            }
        });
    }

    static void postNotification(String title, String body) {
        Notifications.reminder(app, title, body);
    }

    static void scheduleReminders(String json) {
        Reminders.schedule(app, json);
    }

    /** The home-screen widget's state changed: store it, redraw the widgets. */
    static void publishSnapshot(String json) {
        main.post(() -> PatchWidget.publish(app, json));
    }

    /** Google's code scanner: no camera permission, the camera UI is Play services'. */
    static void scanQr() {
        main.post(() -> {
            Context context = activity.get() != null ? activity.get() : app;
            GmsBarcodeScannerOptions options = new GmsBarcodeScannerOptions.Builder()
                    .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                    .build();
            GmsBarcodeScanning.getClient(context, options).startScan()
                    .addOnSuccessListener(code -> {
                        if (code.getRawValue() != null) {
                            nativeScanned(code.getRawValue());
                        }
                    })
                    .addOnFailureListener(e -> {
                        Log.w(TAG, "QR scanner unavailable", e);
                        Toast.makeText(app, "Scanner unavailable: type the hub address and token instead", Toast.LENGTH_LONG).show();
                    });
        });
    }

    static void checkUpdate() {
        main.post(() -> Updater.check(app));
    }

    static boolean batteryUnrestricted() {
        PowerManager power = app.getSystemService(PowerManager.class);
        return power != null && power.isIgnoringBatteryOptimizations(app.getPackageName());
    }

    static void openBatterySettings() {
        main.post(() -> {
            Intent intent = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                    Uri.parse("package:" + app.getPackageName()));
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            try {
                app.startActivity(intent);
            } catch (RuntimeException e) {
                Log.w(TAG, "no app battery settings", e);
                Toast.makeText(app, "App settings unavailable", Toast.LENGTH_SHORT).show();
            }
        });
    }

    static void requestBatteryUnrestricted() {
        main.post(() -> {
            Intent intent = new Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                    Uri.parse("package:" + app.getPackageName()));
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            try {
                app.startActivity(intent);
            } catch (RuntimeException e) {
                // Some vendors drop the direct prompt; fall back to the list.
                Intent list = new Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS);
                list.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                try {
                    app.startActivity(list);
                } catch (RuntimeException e2) {
                    Log.w(TAG, "no battery optimization settings", e2);
                }
            }
        });
    }

    static void copyText(String text) {
        Runnable copy = () -> clipboard.setPrimaryClip(ClipData.newPlainText("Patch", text));
        if (Looper.myLooper() == Looper.getMainLooper()) {
            copy.run();
        } else {
            main.post(copy);
        }
    }

    static String clipboardText() {
        // Binder call, fine off the main thread. Android 10+ returns null
        // unless one of our windows has focus.
        ClipData clip = clipboard.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            return null;
        }
        CharSequence text = clip.getItemAt(0).coerceToText(app);
        return text == null ? null : text.toString();
    }

    static void imeState(int kind, int mode, int seq, String text, int selStart, int selEnd, int compStart, int compEnd,
            int inputType, int action) {
        main.post(() -> {
            PatchSurfaceView view = views[kind];
            if (view != null) {
                view.imeState(mode, seq, text, selStart, selEnd, compStart, compEnd, inputType, action);
            }
        });
    }

    static void frameDone(int kind) {
        main.post(() -> {
            Runnable waiter = frameWaiters[kind].peek();
            if (waiter != null) {
                waiter.run();
            }
        });
    }

    static void backResult(boolean consumed) {
        main.post(() -> {
            MainActivity a = activity.get();
            if (!consumed && a != null && !a.isFinishing()) {
                a.moveTaskToBack(true);
            }
        });
    }

    static void setLook(int value) {
        look = value;
        main.post(() -> {
            MainActivity a = activity.get();
            if (a != null) {
                a.applyLook(value);
            }
        });
    }

    // ---- Implemented in Rust ---------------------------------------------

    static native void nativeInit(Context context, String filesDir, float density);

    static native void nativeSurfaceChanged(int kind, Surface surface, int width, int height);

    /** Blocks until Rust no longer draws into the surface. */
    static native void nativeSurfaceDestroyed(int kind);

    static native void nativeRedrawNeeded(int kind);

    /** The view is gone for good; Rust drops its window. */
    static native void nativeViewGone(int kind);

    static native void nativeTouch(int kind, int action, float x, float y);

    static native void nativeKey(int kind, int keyCode, int unicodeChar, boolean down, int seq);

    static native void nativeIme(int kind, int op, String text, int a, int b, int seq);

    /** Answered asynchronously with {@link #backResult}. */
    static native void nativeBack();

    /** System bar and keyboard insets, in pixels. */
    static native void nativeInsets(float top, float bottom);

    static native void nativeDensity(float density);

    /** Pet origin relative to the usable screen, in logical pixels. */
    static native void nativeOverlayPosition(float x, float y);

    /** 0 idle, ±1 held, ±2 flying, ±3 bump, 4 landing, 5 dizzy. Sign is facing. */
    static native void nativePetPlay(int state);

    static native void nativeOverlayAllowed(boolean allowed);

    /** The user hid the overlay from its notification. */
    static native void nativeOverlayClosed();

    /** Saves; blocks briefly. */
    static native void nativePause();

    static native void nativeSyncNow();

    static native void nativeOpenTask(String taskId);
    static native void nativeCompleteTask(String taskId);

    /** The widget's "+": open the editor on a new task. */
    static native void nativeNewTask();

    /** Text of a QR code scanned for pairing. */
    static native void nativeScanned(String text);
}
