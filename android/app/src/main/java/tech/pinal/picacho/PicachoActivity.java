package tech.pinal.picacho;

import android.annotation.TargetApi;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.os.Process;
import android.view.Display;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.View;
import android.view.WindowInsets;
import java.util.Arrays;
import android.window.BackEvent;
import android.window.OnBackAnimationCallback;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;
import dev.gpui.mobile.GpuiInputActivity;

/**
 * Picacho's activity: a NativeActivity whose window is all Rust
 * (android_main in src/android.rs). Through gpui-mobile's GpuiInputActivity it
 * has the software keyboard: a hidden text field's InputConnection carries
 * typing (and composing) to GPUI's text fields.
 *
 * <p>GPUI's Android platform starts once per process. When the activity is
 * gone but the process lives on (its {@link MailService} keeps it running),
 * an activity created later can't show a window; it restarts the process
 * instead, through {@link RestartActivity}, and the new process opens
 * Picacho as usual.
 *
 * <p>Back: while Picacho has somewhere to go back to (a message to close, a
 * screen below, a sheet), Rust registers a back callback (setBackEnabled) and
 * Back goes there, a back swipe dragging the screen as it goes (Android 14
 * and later). At Picacho's first screen the callback is off, so Android does
 * its own Back: home, with its predictive animation. Before Android 13 Back
 * reaches Rust as a key (see leave).
 */
public final class PicachoActivity extends GpuiInputActivity {
    /** An activity's window already ran in this process. */
    private static boolean started;
    /** The activity showing Picacho's window, for Rust's calls. */
    private static volatile PicachoActivity current;
    /** The back callback, and whether it is registered. UI thread only. */
    private OnBackInvokedCallback back;
    private boolean backRegistered;
    /** The insets last handed to Rust: top, bottom, left, right, keyboard. */
    private float[] insets;
    /** How long after the last frame the window stops preferring the fastest mode. */
    private static final long FAST_FOR_MS = 1000;
    /** The display's fastest mode at this resolution; 0 until a surface is created. */
    private int fastestMode;
    /** The window prefers it (Picacho drew a frame within FAST_FOR_MS). UI thread. */
    private boolean fast;
    private final Runnable slowDown = this::slowDown;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        Native.attach(getApplicationContext());
        if (started) {
            // android_main returns at once in this case.
            super.onCreate(savedInstanceState);
            restartProcess();
            return;
        }
        started = true;
        Native.beforeActivity();
        // A tapped notification's target, before the window starts: it opens
        // there once it's up.
        if (savedInstanceState == null) {
            opened(getIntent());
        }
        super.onCreate(savedInstanceState);
        current = this;
        // Android 15 and later draw apps edge to edge: the window runs under
        // the status and navigation bars, and the keyboard no longer resizes
        // it. NativeActivity's content rect (gpui-mobile's safe area) then
        // covers the whole screen, so the bars and keyboard are told apart.
        getWindow().getDecorView().setOnApplyWindowInsetsListener(this::onInsets);
        Notifier.requestPermission(this);
    }

    /** Hands the system bars', cutout's and keyboard's insets to Rust, in points. */
    private WindowInsets onInsets(View view, WindowInsets windowInsets) {
        float top, bottom, left, right, keyboard = 0;
        if (Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets bars = windowInsets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            top = bars.top;
            bottom = bars.bottom;
            left = bars.left;
            right = bars.right;
            keyboard = windowInsets.getInsets(WindowInsets.Type.ime()).bottom;
        } else {
            top = windowInsets.getSystemWindowInsetTop();
            bottom = windowInsets.getSystemWindowInsetBottom();
            left = windowInsets.getSystemWindowInsetLeft();
            right = windowInsets.getSystemWindowInsetRight();
        }
        float density = getResources().getDisplayMetrics().density;
        float[] now = {top / density, bottom / density, left / density, right / density,
                keyboard / density};
        if (!Arrays.equals(now, insets)) {
            insets = now;
            Native.insets(now[0], now[1], now[2], now[3], now[4]);
        }
        // The window's own handling, as without a listener.
        return view.onApplyWindowInsets(windowInsets);
    }

    /**
     * Asks for the display's highest refresh rate while Picacho draws.
     * gpui-mobile asks too, but its lookup of the NDK function (dlsym with
     * RTLD_DEFAULT) can't see libandroid from an app's library, so its
     * request never reaches Android, and without one, systems such as
     * ColorOS keep a NativeActivity at 60 Hz. The request is the surface's:
     * asked again for each new surface, and Android only honors it while
     * Picacho is drawing, so an idle screen can still slow down.
     */
    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        super.surfaceCreated(holder);
        requestHighFrameRate(holder.getSurface());
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        super.surfaceChanged(holder, format, width, height);
        requestHighFrameRate(holder.getSurface());
    }

    private void requestHighFrameRate(Surface surface) {
        if (Build.VERSION.SDK_INT < 30 || surface == null || !surface.isValid()) {
            return;
        }
        Display display = getDisplay();
        if (display == null) {
            return;
        }
        // The fastest mode at the current resolution: switching among those
        // is seamless.
        Display.Mode current = display.getMode();
        float rate = current.getRefreshRate();
        for (Display.Mode mode : display.getSupportedModes()) {
            if (mode.getPhysicalWidth() == current.getPhysicalWidth()
                    && mode.getPhysicalHeight() == current.getPhysicalHeight()) {
                rate = Math.max(rate, mode.getRefreshRate());
            }
            if (Build.VERSION.SDK_INT >= 31 && mode.getModeId() == current.getModeId()) {
                for (float alternative : mode.getAlternativeRefreshRates()) {
                    rate = Math.max(rate, alternative);
                }
            }
        }
        rememberFastestMode(display, current);
        try {
            if (Build.VERSION.SDK_INT >= 31) {
                surface.setFrameRate(rate, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT,
                        Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            } else {
                surface.setFrameRate(rate, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT);
            }
        } catch (RuntimeException e) {
            android.util.Log.w("picacho", "could not ask for " + rate + " Hz", e);
        }
    }

    /**
     * The fastest display mode at the current resolution, which the window
     * prefers while Picacho draws (see {@link #drawing}). Some systems
     * (ColorOS) cap apps they don't know at 60 Hz whatever their surfaces
     * ask, but honor a window's own preference.
     */
    private void rememberFastestMode(Display display, Display.Mode current) {
        Display.Mode fastest = current;
        for (Display.Mode mode : display.getSupportedModes()) {
            if (mode.getPhysicalWidth() == current.getPhysicalWidth()
                    && mode.getPhysicalHeight() == current.getPhysicalHeight()
                    && mode.getRefreshRate() > fastest.getRefreshRate()) {
                fastest = mode;
            }
        }
        fastestMode = fastest.getModeId();
        if (fast) {
            preferMode(fastestMode);
        }
    }

    /**
     * Called from Rust, on any thread, while Picacho draws frames (a scroll,
     * an animation), at most a few times a second. The window prefers the
     * fastest display mode until it hasn't been called for {@link #FAST_FOR_MS},
     * then leaves the refresh rate to the system again, so a still screen
     * can slow down and save power whatever the system's own policy.
     */
    public static void drawing() {
        PicachoActivity activity = current;
        if (activity != null) {
            activity.runOnUiThread(activity::keepFast);
        }
    }

    private void keepFast() {
        if (isDestroyed()) {
            return;
        }
        View decor = getWindow().getDecorView();
        decor.removeCallbacks(slowDown);
        decor.postDelayed(slowDown, FAST_FOR_MS);
        if (!fast) {
            fast = true;
            preferMode(fastestMode);
        }
    }

    private void slowDown() {
        fast = false;
        preferMode(0);
    }

    /** The window's preferred display mode; 0 for none. UI thread. */
    private void preferMode(int mode) {
        android.view.WindowManager.LayoutParams params = getWindow().getAttributes();
        if (params.preferredDisplayModeId != mode) {
            params.preferredDisplayModeId = mode;
            getWindow().setAttributes(params);
        }
    }

    /** The activity showing Picacho's window, if there is one. */
    static PicachoActivity current() {
        return current;
    }

    @Override
    protected void onResume() {
        super.onResume();
        // NFC reader mode is the resumed activity's, and ends with a pause.
        SecurityKeys.resumed(this);
    }

    /**
     * Opened again while it runs: from the launcher, a notification, or a
     * browser sign-in's last page (the app's own scheme), which brings
     * Picacho back over the browser. A notification's target is opened.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        opened(intent);
    }

    /** Hands a tapped notification's target to Rust, once. */
    private static void opened(Intent intent) {
        String target = Notifier.targetOf(intent);
        if (target != null) {
            intent.removeExtra(Notifier.EXTRA_OPEN);
            Native.open(target);
        }
    }

    @Override
    protected void onDestroy() {
        if (current == this) {
            current = null;
        }
        super.onDestroy();
    }

    /**
     * Android's own Back at Picacho's first screen ends by asking the activity
     * to finish (it only moves launcher activities to the background itself,
     * and the launcher entry is an alias). Finished, its window couldn't come
     * back without restarting the process; so it goes to the background, as a
     * launcher's Back does, after the system's back-to-home animation. Picacho
     * itself finishes with finish().
     */
    @Override
    public void finishAfterTransition() {
        if (isTaskRoot()) {
            moveTaskToBack(true);
        } else {
            super.finishAfterTransition();
        }
    }

    /** Called from Rust, on any thread, when Picacho can or can't go back. */
    public static void setBackEnabled(boolean enabled) {
        PicachoActivity activity = current;
        if (activity != null && Build.VERSION.SDK_INT >= 33) {
            activity.runOnUiThread(() -> activity.applyBack(enabled));
        }
    }

    /**
     * Called from Rust when Back reaches Picacho's first screen as a key
     * (before Android 13): Picacho goes to the background, as Back does
     * there. Later Androids do it themselves (the callback is off then).
     */
    public static void leave() {
        PicachoActivity activity = current;
        if (activity != null && Build.VERSION.SDK_INT < 33) {
            activity.runOnUiThread(() -> activity.moveTaskToBack(true));
        }
    }

    @TargetApi(33)
    private void applyBack(boolean enabled) {
        if (isDestroyed() || enabled == backRegistered) {
            return;
        }
        if (back == null) {
            back = Build.VERSION.SDK_INT >= 34 ? new AnimatedBack() : () -> Native.back(2, 1f);
        }
        OnBackInvokedDispatcher dispatcher = getOnBackInvokedDispatcher();
        if (enabled) {
            dispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, back);
        } else {
            dispatcher.unregisterOnBackInvokedCallback(back);
        }
        backRegistered = enabled;
    }

    /** A back swipe's progress, for the screen to follow (Android 14 and later). */
    @TargetApi(34)
    private static final class AnimatedBack implements OnBackAnimationCallback {
        @Override
        public void onBackStarted(BackEvent event) {
            Native.back(0, event.getProgress());
        }

        @Override
        public void onBackProgressed(BackEvent event) {
            Native.back(1, event.getProgress());
        }

        @Override
        public void onBackInvoked() {
            Native.back(2, 1f);
        }

        @Override
        public void onBackCancelled() {
            Native.back(3, 0f);
        }
    }

    private void restartProcess() {
        Native.shutdown();
        Intent restart = new Intent(this, RestartActivity.class)
                .putExtra(RestartActivity.EXTRA_PID, Process.myPid())
                // A tapped notification's target goes to the new process.
                .putExtra(Notifier.EXTRA_OPEN, Notifier.targetOf(getIntent()))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NO_ANIMATION);
        startActivity(restart);
        finish();
    }
}
