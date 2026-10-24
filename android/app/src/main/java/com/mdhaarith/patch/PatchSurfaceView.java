package com.mdhaarith.patch;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.os.Build;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.SpannableStringBuilder;
import android.text.TextUtils;
import android.view.ActionMode;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.Menu;
import android.view.MenuItem;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup.LayoutParams;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.PopupWindow;

/**
 * A surface Rust renders into, for the activity ({@link PatchNative#MAIN}) or
 * the floating overlay ({@link PatchNative#OVERLAY}). It forwards the surface
 * lifecycle, touches and keys, and is the IME's text editor: it keeps a mirror
 * of the focused Slint text field so the keyboard's queries are answered
 * immediately, and forwards every edit to Rust, which echoes the real state
 * back through {@link #imeState}.
 */
final class PatchSurfaceView extends SurfaceView implements SurfaceHolder.Callback2 {
    /** Overlay windows have to be made focusable before the IME can attach. */
    interface FocusHost {
        boolean setImeFocus(boolean focus);
    }

    final int kind;
    private final InputMethodManager imm;
    private FocusHost focusHost;

    // IME mirror. Main thread only.
    final Editable editable = new SpannableStringBuilder();
    private int inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
    private int imeAction = EditorInfo.IME_ACTION_DONE;
    private int sentSeq;
    private boolean imeWanted;
    private boolean showPending;
    private int batchDepth;

    private int primaryPointer = -1;
    private float touchX, touchY;
    private ActionMode textActions;
    private PopupWindow overlayTextActions;
    private final Runnable longPress = this::handleLongPress;

    private void handleLongPress() {
        PatchNative.nativeTouch(kind, PatchNative.TOUCH_LONG_PRESS, touchX, touchY);
    }

    PatchSurfaceView(Context context, int kind) {
        super(context);
        this.kind = kind;
        imm = context.getSystemService(InputMethodManager.class);
        SurfaceHolder holder = getHolder();
        holder.addCallback(this);
        if (kind == PatchNative.OVERLAY) {
            // Transparent around the bubble and the panel's rounded corners,
            // composited above the (also transparent) overlay window.
            holder.setFormat(PixelFormat.TRANSLUCENT);
            setZOrderOnTop(true);
        } else {
            holder.setFormat(PixelFormat.RGBX_8888);
        }
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    void setFocusHost(FocusHost host) {
        focusHost = host;
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        PatchNative.register(kind, this);
    }

    @Override
    protected void onDetachedFromWindow() {
        removeCallbacks(longPress);
        finishTextActions();
        PatchNative.unregister(kind, this);
        super.onDetachedFromWindow();
    }

    // ---- Surface ------------------------------------------------------------

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        // surfaceChanged always follows with the size.
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        PatchNative.nativeSurfaceChanged(kind, holder.getSurface(), width, height);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        PatchNative.nativeSurfaceDestroyed(kind);
    }

    @Override
    public void surfaceRedrawNeeded(SurfaceHolder holder) {
        // Only the async variant is used.
    }

    @Override
    public void surfaceRedrawNeededAsync(SurfaceHolder holder, Runnable drawingFinished) {
        PatchNative.awaitFrame(kind, drawingFinished);
    }

    // ---- Touch ----------------------------------------------------------------

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        if (routedTouches) {
            return false; // The overlay frame routes its touches itself.
        }
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN && !hasFocus()) {
            requestFocus();
        }
        forwardTouch(event, 0, 0);
        if (event.getActionMasked() == MotionEvent.ACTION_UP) {
            performClick();
        }
        return true;
    }

    @Override
    public boolean performClick() {
        return super.performClick();
    }

    /**
     * Forwards the first finger only; Slint is driven like a mouse. Positions
     * are in surface pixels, shifted by ({@code dx}, {@code dy}).
     */
    void forwardTouch(MotionEvent event, float dx, float dy) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                primaryPointer = event.getPointerId(0);
                touchX = event.getX(0) + dx;
                touchY = event.getY(0) + dy;
                finishTextActions();
                postDelayed(longPress, ViewConfiguration.getLongPressTimeout());
                send(PatchNative.TOUCH_DOWN, event, 0, dx, dy);
                break;
            case MotionEvent.ACTION_MOVE: {
                int index = event.findPointerIndex(primaryPointer);
                if (index >= 0) {
                    float x = event.getX(index) + dx;
                    float y = event.getY(index) + dy;
                    int slop = ViewConfiguration.get(getContext()).getScaledTouchSlop();
                    if (Math.abs(x - touchX) > slop || Math.abs(y - touchY) > slop) {
                        removeCallbacks(longPress);
                    }
                    send(PatchNative.TOUCH_MOVE, event, index, dx, dy);
                }
                break;
            }
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_UP: {
                int index = event.getActionIndex();
                if (event.getPointerId(index) == primaryPointer) {
                    removeCallbacks(longPress);
                    send(PatchNative.TOUCH_UP, event, index, dx, dy);
                    primaryPointer = -1;
                }
                break;
            }
            case MotionEvent.ACTION_CANCEL:
                removeCallbacks(longPress);
                if (primaryPointer != -1) {
                    PatchNative.nativeTouch(kind, PatchNative.TOUCH_CANCEL, 0, 0);
                    primaryPointer = -1;
                }
                break;
            default:
                break;
        }
    }

    private void send(int action, MotionEvent event, int index, float dx, float dy) {
        PatchNative.nativeTouch(kind, action, event.getX(index) + dx, event.getY(index) + dy);
    }

    /** A synthetic tap, for the overlay's drag handles. */
    /** Set by {@link OverlayService}, whose frame drags and forwards touches itself. */
    boolean routedTouches;

    void tap(float x, float y) {
        PatchNative.nativeTouch(kind, PatchNative.TOUCH_DOWN, x, y);
        PatchNative.nativeTouch(kind, PatchNative.TOUCH_UP, x, y);
    }

    // ---- Keys -----------------------------------------------------------------

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        return key(event, true) || super.onKeyDown(keyCode, event);
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        return key(event, false) || super.onKeyUp(keyCode, event);
    }

    private boolean key(KeyEvent event, boolean down) {
        int code = event.getKeyCode();
        if (code == KeyEvent.KEYCODE_BACK || event.isSystem()) {
            return false;
        }
        int unicode = event.getUnicodeChar(event.getMetaState());
        if (down) {
            mirrorKey(code, unicode);
        }
        PatchNative.nativeKey(kind, code, unicode, down, ++sentSeq);
        return true;
    }

    /** Keeps the mirror in step with keys that edit (IME DEL, hardware keyboards). */
    private void mirrorKey(int code, int unicode) {
        int start = Selection.getSelectionStart(editable);
        int end = Selection.getSelectionEnd(editable);
        if (start < 0 || end < 0) {
            return;
        }
        int lo = Math.min(start, end);
        int hi = Math.max(start, end);
        if (code == KeyEvent.KEYCODE_DEL) {
            if (lo != hi) {
                editable.delete(lo, hi);
            } else if (lo > 0) {
                editable.delete(Character.offsetByCodePoints(editable, lo, -1), lo);
            }
        } else if (code == KeyEvent.KEYCODE_FORWARD_DEL) {
            if (lo != hi) {
                editable.delete(lo, hi);
            } else if (hi < editable.length()) {
                editable.delete(hi, Character.offsetByCodePoints(editable, hi, 1));
            }
        } else if (unicode > 0 && !Character.isISOControl(unicode)) {
            editable.replace(lo, hi, new String(Character.toChars(unicode)));
        }
    }

    // ---- IME ------------------------------------------------------------------

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        out.inputType = inputType;
        out.imeOptions = imeAction | EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN;
        out.initialSelStart = Selection.getSelectionStart(editable);
        out.initialSelEnd = Selection.getSelectionEnd(editable);
        if (Build.VERSION.SDK_INT >= 30) {
            out.setInitialSurroundingText(editable);
        }
        batchDepth = 0;
        return new PatchInputConnection(this);
    }

    /** An edit made by the IME through the connection; already in the mirror. */
    void imeEdit(int op, String text, int a, int b) {
        PatchNative.nativeIme(kind, op, text, a, b, ++sentSeq);
        if (batchDepth == 0) {
            reportSelection();
        }
    }

    void beginBatch() {
        batchDepth++;
    }

    void endBatch() {
        if (batchDepth > 0 && --batchDepth == 0) {
            reportSelection();
        }
    }

    int batchDepth() {
        return batchDepth;
    }

    private void reportSelection() {
        imm.updateSelection(this, Selection.getSelectionStart(editable), Selection.getSelectionEnd(editable),
                BaseInputConnection.getComposingSpanStart(editable), BaseInputConnection.getComposingSpanEnd(editable));
        if (textActions != null) {
            textActions.invalidate();
        }
    }

    /** The focused text field's state from Rust. */
    void imeState(int mode, int seq, String text, int selStart, int selEnd, int compStart, int compEnd, int type, int action) {
        if (mode == PatchNative.IME_HIDE) {
            imeWanted = false;
            finishTextActions();
            hideIme();
            return;
        }
        boolean restart = mode >= PatchNative.IME_SHOW || type != inputType || action != imeAction;
        if (!restart && seq < sentSeq) {
            return; // Our newer edits are still on their way; their echo follows.
        }
        inputType = type;
        imeAction = action;
        boolean textChanged = !TextUtils.equals(editable, text);
        boolean selectionChanged = selStart != Selection.getSelectionStart(editable)
                || selEnd != Selection.getSelectionEnd(editable)
                || compStart != BaseInputConnection.getComposingSpanStart(editable)
                || compEnd != BaseInputConnection.getComposingSpanEnd(editable);
        if (textChanged || selectionChanged || restart) {
            setMirror(text, selStart, selEnd, compStart, compEnd);
        }
        if (restart || textChanged) {
            // The IME's idea of the text is wrong (another field, filtered
            // input, a field the app cleared): start over.
            imm.restartInput(this);
        } else if (selectionChanged) {
            reportSelection();
        }
        if (mode >= PatchNative.IME_SHOW) {
            imeWanted = true;
            showIme();
        }
        if (mode == PatchNative.IME_SHOW_ACTIONS || (selStart != selEnd && selectionChanged)) {
            showTextActions();
        } else if (selStart == selEnd) {
            finishTextActions();
        }
    }

    private void showTextActions() {
        // Service overlay windows have no DecorView to host an ActionMode.
        if (kind == PatchNative.OVERLAY) {
            showOverlayTextActions();
            return;
        }
        if (textActions != null) {
            textActions.invalidate();
            textActions.invalidateContentRect();
            return;
        }
        textActions = startActionMode(new ActionMode.Callback2() {
            @Override
            public boolean onCreateActionMode(ActionMode mode, Menu menu) {
                menu.add(0, android.R.id.copy, 0, android.R.string.copy).setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM);
                menu.add(0, android.R.id.cut, 1, android.R.string.cut).setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM);
                menu.add(0, android.R.id.paste, 2, android.R.string.paste).setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM);
                menu.add(0, android.R.id.selectAll, 3, android.R.string.selectAll).setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM);
                return true;
            }

            @Override
            public boolean onPrepareActionMode(ActionMode mode, Menu menu) {
                boolean selected = Selection.getSelectionStart(editable) != Selection.getSelectionEnd(editable);
                menu.findItem(android.R.id.copy).setEnabled(selected);
                menu.findItem(android.R.id.cut).setEnabled(selected);
                menu.findItem(android.R.id.selectAll).setEnabled(editable.length() > 0);
                return true;
            }

            @Override
            public boolean onActionItemClicked(ActionMode mode, MenuItem item) {
                boolean handled = new PatchInputConnection(PatchSurfaceView.this).performContextMenuAction(item.getItemId());
                if (item.getItemId() != android.R.id.selectAll) {
                    mode.finish();
                }
                return handled;
            }

            @Override
            public void onDestroyActionMode(ActionMode mode) {
                textActions = null;
            }

            @Override
            public void onGetContentRect(ActionMode mode, View view, Rect out) {
                out.set(Math.round(touchX), Math.round(touchY), Math.round(touchX) + 1, Math.round(touchY) + 1);
            }
        }, ActionMode.TYPE_FLOATING);
    }

    private void finishTextActions() {
        if (textActions != null) {
            textActions.finish();
        }
        if (overlayTextActions != null) {
            overlayTextActions.dismiss();
            overlayTextActions = null;
        }
    }

    @SuppressLint("RtlHardcoded") // Popup positions use physical screen coordinates.
    private void showOverlayTextActions() {
        finishTextActions();
        LinearLayout bar = new LinearLayout(getContext());
        bar.setBackgroundColor(0xff303030);
        int[] ids = {android.R.id.copy, android.R.id.cut, android.R.id.paste, android.R.id.selectAll};
        int[] labels = {android.R.string.copy, android.R.string.cut, android.R.string.paste, android.R.string.selectAll};
        boolean selected = Selection.getSelectionStart(editable) != Selection.getSelectionEnd(editable);
        for (int i = 0; i < ids.length; i++) {
            int id = ids[i];
            Button button = new Button(getContext(), null, android.R.attr.borderlessButtonStyle);
            button.setText(labels[i]);
            button.setMinWidth(0);
            button.setTextSize(12);
            button.setEnabled(id != android.R.id.copy && id != android.R.id.cut || selected);
            button.setOnClickListener(v -> {
                new PatchInputConnection(this).performContextMenuAction(id);
                finishTextActions();
                if (id == android.R.id.selectAll) {
                    showOverlayTextActions();
                }
            });
            bar.addView(button);
        }
        overlayTextActions = new PopupWindow(bar, LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT);
        overlayTextActions.setInputMethodMode(PopupWindow.INPUT_METHOD_NOT_NEEDED);
        overlayTextActions.setElevation(8 * getResources().getDisplayMetrics().density);
        bar.measure(MeasureSpec.UNSPECIFIED, MeasureSpec.UNSPECIFIED);
        int[] location = new int[2];
        getLocationOnScreen(location);
        overlayTextActions.showAtLocation(this, Gravity.TOP | Gravity.LEFT,
                location[0] + Math.max(0, Math.min(Math.round(touchX), getWidth() - bar.getMeasuredWidth())),
                location[1] + Math.max(0, Math.round(touchY) - bar.getMeasuredHeight()));
    }

    private void setMirror(String text, int selStart, int selEnd, int compStart, int compEnd) {
        int len = text.length();
        compStart = Math.min(compStart, len);
        compEnd = Math.min(compEnd, len);
        SpannableStringBuilder fresh = new SpannableStringBuilder(text);
        if (compStart >= 0 && compEnd > compStart) {
            // setComposingSpans marks a whole Spannable; the span survives
            // being copied in at the composing range.
            SpannableStringBuilder composing = new SpannableStringBuilder(text, compStart, compEnd);
            BaseInputConnection.setComposingSpans(composing);
            fresh.replace(compStart, compEnd, composing);
        }
        editable.clear();
        editable.clearSpans();
        editable.append(fresh);
        Selection.setSelection(editable, clamp(selStart, len), clamp(selEnd, len));
    }

    private static int clamp(int v, int len) {
        return Math.max(0, Math.min(v, len));
    }

    private void showIme() {
        if (focusHost != null && !focusHost.setImeFocus(true)) {
            imeWanted = false;
            showPending = false;
            return;
        }
        if (!hasFocus()) {
            requestFocus();
        }
        if (hasWindowFocus()) {
            imm.showSoftInput(this, 0);
        } else {
            showPending = true; // Overlay windows only get focus a moment later.
        }
    }

    private void hideIme() {
        showPending = false;
        if (getWindowToken() != null) {
            imm.hideSoftInputFromWindow(getWindowToken(), 0);
        }
        if (focusHost != null) {
            focusHost.setImeFocus(false);
        }
    }

    /** The overlay gave up focus (touch outside, keyboard dismissed). */
    void imeDismissed() {
        finishTextActions();
        showPending = false;
    }

    boolean imeWanted() {
        return imeWanted;
    }

    @Override
    public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (hasWindowFocus && showPending && imeWanted) {
            showPending = false;
            requestFocus();
            imm.showSoftInput(this, 0);
        }
    }
}
