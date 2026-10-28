package com.mdhaarith.patch;

import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.drawable.GradientDrawable;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.Toast;
import androidx.annotation.NonNull;
import androidx.camera.core.CameraSelector;
import androidx.camera.core.ImageAnalysis;
import androidx.camera.core.ImageProxy;
import androidx.camera.core.Preview;
import androidx.camera.lifecycle.ProcessCameraProvider;
import androidx.camera.view.PreviewView;
import androidx.lifecycle.Lifecycle;
import androidx.lifecycle.LifecycleOwner;
import androidx.lifecycle.LifecycleRegistry;
import com.google.common.util.concurrent.ListenableFuture;
import com.google.mlkit.vision.barcode.BarcodeScanner;
import com.google.mlkit.vision.barcode.BarcodeScannerOptions;
import com.google.mlkit.vision.barcode.BarcodeScanning;
import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.common.InputImage;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** A TextureView clipped to the Slint scan card; owns only its camera use cases. */
final class QrScanner implements LifecycleOwner, AutoCloseable {
    private final MainActivity activity;
    private final FrameLayout parent;
    private final FrameLayout card;
    private final PreviewView previewView;
    private final LifecycleRegistry lifecycle = new LifecycleRegistry(this);
    private final ExecutorService analysisThread = Executors.newSingleThreadExecutor();
    private final BarcodeScanner decoder = BarcodeScanning.getClient(new BarcodeScannerOptions.Builder()
            .setBarcodeFormats(Barcode.FORMAT_QR_CODE).build());
    private ProcessCameraProvider provider;
    private Preview preview;
    private ImageAnalysis analysis;
    private volatile boolean closed;
    private boolean delivered;

    QrScanner(MainActivity activity, FrameLayout parent) {
        this.activity = activity;
        this.parent = parent;
        card = new FrameLayout(activity);
        GradientDrawable background = new GradientDrawable();
        background.setColor(0xFF141518);
        background.setCornerRadius(20 * activity.getResources().getDisplayMetrics().density);
        card.setBackground(background);
        card.setClipToOutline(true);
        previewView = new PreviewView(activity);
        previewView.setImplementationMode(PreviewView.ImplementationMode.COMPATIBLE);
        previewView.setScaleType(PreviewView.ScaleType.FILL_CENTER);
        card.addView(previewView, new FrameLayout.LayoutParams(-1, -1));
        card.addView(new View(activity) {
            private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
            @Override protected void onDraw(Canvas canvas) {
                float d = getResources().getDisplayMetrics().density;
                float size = Math.min(getWidth(), getHeight()) * 0.7f;
                float x = (getWidth() - size) / 2, y = (getHeight() - size) / 2;
                paint.setColor(Color.WHITE);
                paint.setStyle(Paint.Style.STROKE);
                paint.setStrokeWidth(2 * d);
                canvas.drawRoundRect(x, y, x + size, y + size, 26 * d, 26 * d, paint);
            }
        }, new FrameLayout.LayoutParams(-1, -1));
        parent.addView(card, new FrameLayout.LayoutParams(1, 1));
        lifecycle.setCurrentState(Lifecycle.State.CREATED);
    }

    @NonNull @Override public Lifecycle getLifecycle() { return lifecycle; }

    void place(int[] rect, int left, int top) {
        float density = activity.getResources().getDisplayMetrics().density;
        FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                Math.max(1, Math.round(rect[2] * density)), Math.max(1, Math.round(rect[3] * density)));
        lp.leftMargin = left + Math.round(rect[0] * density);
        lp.topMargin = top + Math.round(rect[1] * density);
        card.setLayoutParams(lp);
        // Hide under the header so scrolling never covers its navigation actions.
        int bar = activity.getResources().getIdentifier("status_bar_height", "dimen", "android");
        int safeTop = bar == 0 ? 0 : activity.getResources().getDimensionPixelSize(bar);
        card.setVisibility(lp.topMargin < safeTop + Math.round(76 * density) ? View.INVISIBLE : View.VISIBLE);
    }

    void start() {
        ListenableFuture<ProcessCameraProvider> future = ProcessCameraProvider.getInstance(activity);
        future.addListener(() -> {
            if (closed) return;
            try {
                provider = future.get();
                preview = new Preview.Builder().build();
                preview.setSurfaceProvider(previewView.getSurfaceProvider());
                analysis = new ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build();
                analysis.setAnalyzer(analysisThread, this::analyze);
                lifecycle.setCurrentState(Lifecycle.State.RESUMED);
                provider.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis);
            } catch (Exception e) {
                android.util.Log.w("Patch", "Camera scanner unavailable", e);
                Toast.makeText(activity, "Camera unavailable. Enter the address and code below.", Toast.LENGTH_LONG).show();
                activity.stopScan();
            }
        }, activity.getMainExecutor());
    }

    @androidx.camera.core.ExperimentalGetImage
    private void analyze(ImageProxy frame) {
        if (closed || frame.getImage() == null) { frame.close(); return; }
        InputImage image = InputImage.fromMediaImage(frame.getImage(), frame.getImageInfo().getRotationDegrees());
        try {
            decoder.process(image)
                    .addOnSuccessListener(activity.getMainExecutor(), codes -> {
                        if (closed || delivered) return;
                        for (Barcode code : codes) {
                            String value = code.getRawValue();
                            if (value != null && value.startsWith("patch://pair?")) {
                                delivered = true;
                                PatchNative.nativeScanned(value);
                                activity.stopScan();
                                return;
                            }
                        }
                    })
                    .addOnCompleteListener(task -> frame.close());
        } catch (RuntimeException e) { frame.close(); }
    }

    @Override public void close() {
        if (closed) return;
        closed = true;
        if (analysis != null) analysis.clearAnalyzer();
        if (provider != null && preview != null && analysis != null) provider.unbind(preview, analysis);
        lifecycle.setCurrentState(Lifecycle.State.DESTROYED);
        parent.removeView(card);
        analysisThread.shutdown();
        decoder.close();
    }
}
