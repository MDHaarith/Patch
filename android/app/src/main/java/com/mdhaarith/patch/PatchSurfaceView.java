package com.mdhaarith.patch;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.PixelFormat;
import android.graphics.Typeface;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.os.Build;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.SpannableStringBuilder;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.ViewConfiguration;
import android.view.ViewGroup.LayoutParams;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.LinearLayout;
import android.widget.PopupWindow;
import android.widget.TextView;

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
    private PopupWindow textActions;
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

    /** Our own copy/paste bar, the same in the activity and the overlay (which has
     * no DecorView for a floating ActionMode): a dark pill above the finger with
     * only the actions that apply. */
    @SuppressLint("RtlHardcoded") // Popup positions use physical screen coordinates.
    private void showTextActions() {
        finishTextActions();
        float dp = getResources().getDisplayMetrics().density;
        int start = Selection.getSelectionStart(editable);
        int end = Selection.getSelectionEnd(editable);
        boolean selected = start != end;
        boolean all = Math.min(start, end) == 0 && Math.max(start, end) == editable.length();
        LinearLayout bar = new LinearLayout(getContext());
        bar.setClipToOutline(true);
        int[] ids = {android.R.id.cut, android.R.id.copy, android.R.id.paste, android.R.id.selectAll};
        int[] labels = {android.R.string.cut, android.R.string.copy, android.R.string.paste, android.R.string.selectAll};
        for (int i = 0; i < ids.length; i++) {
            int id = ids[i];
            if ((id == android.R.id.cut || id == android.R.id.copy) && !selected
                    || id == android.R.id.selectAll && (all || editable.length() == 0)) {
                continue;
            }
            TextView item = new TextView(getContext());
            item.setText(labels[i]);
            item.setSingleLine(true);
            item.setTextSize(14);
            item.setTypeface(Typeface.DEFAULT_BOLD);
            item.setTextColor(0xfff5f5f7);
            item.setGravity(Gravity.CENTER);
            item.setMinHeight(Math.round(40 * dp));
            item.setPadding(Math.round(14 * dp), 0, Math.round(14 * dp), 0);
            item.setBackground(new RippleDrawable(ColorStateList.valueOf(0x33ffffff), null, new ColorDrawable(0xffffffff)));
            item.setOnClickListener(v -> {
                new PatchInputConnection(this).performContextMenuAction(id);
                finishTextActions();
                if (id == android.R.id.selectAll) {
                    showTextActions();
                }
            });
            bar.addView(item);
        }
        if (bar.getChildCount() == 0) {
            return;
        }
        GradientDrawable pill = new GradientDrawable();
        pill.setColor(0xf2242426);
        pill.setCornerRadius(20 * dp);
        pill.setStroke(Math.max(1, Math.round(dp / 2)), 0x22ffffff);
        textActions = new PopupWindow(bar, LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT);
        textActions.setBackgroundDrawable(pill);
        textActions.setInputMethodMode(PopupWindow.INPUT_METHOD_NOT_NEEDED);
        textActions.setElevation(8 * dp);
        bar.measure(MeasureSpec.UNSPECIFIED, MeasureSpec.UNSPECIFIED);
        int w = bar.getMeasuredWidth();
        int h = bar.getMeasuredHeight();
        int gap = Math.round(12 * dp);
        int[] location = new int[2];
        getLocationOnScreen(location);
        int x = Math.max(gap, Math.min(Math.round(touchX) - w / 2, getWidth() - w - gap));
        // Above the finger; below it when there is no room at the top.
        int y = Math.round(touchY) - h - 3 * gap;
        if (y < gap) {
            y = Math.round(touchY) + 2 * gap;
        }
        textActions.showAtLocation(this, Gravity.TOP | Gravity.LEFT, location[0] + x, location[1] + y);
    }

    private void finishTextActions() {
        if (textActions != null) {
            textActions.dismiss();
            textActions = null;
        }
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
