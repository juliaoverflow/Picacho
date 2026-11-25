package tech.pinal.picacho;

import android.annotation.TargetApi;
import android.content.Context;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.Build;
import android.os.CancellationSignal;
import android.util.Log;

/**
 * The app lock's fingerprint or face unlock for Rust (src/android.rs,
 * {@code authenticate_biometrics}), from the framework's BiometricPrompt
 * (Android 10 and later), without the androidx library. Weak biometrics
 * (Class 2, such as most face unlocks) are enough: the prompt only lets the
 * person past Picacho's own lock screen, and unlocks no key.
 *
 * <p>Answers Rust's request with "ok", "cancelled" (the person cancelled,
 * chose the passcode, or the prompt was taken away) or "error:" and why.
 */
final class AppLock {
    private static final String TAG = "picacho";

    private AppLock() {}

    /** Called from Rust: whether a fingerprint or face can unlock now. */
    static boolean available(Context context) {
        if (Build.VERSION.SDK_INT < 29) {
            return false;
        }
        try {
            BiometricManager manager = context.getSystemService(BiometricManager.class);
            if (manager == null) {
                return false;
            }
            int result = Build.VERSION.SDK_INT >= 30
                    ? manager.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_WEAK)
                    : legacyCanAuthenticate(manager);
            return result == BiometricManager.BIOMETRIC_SUCCESS;
        } catch (RuntimeException e) {
            Log.w(TAG, "couldn't ask for biometrics", e);
            return false;
        }
    }

    @SuppressWarnings("deprecation")
    @TargetApi(29)
    private static int legacyCanAuthenticate(BiometricManager manager) {
        return manager.canAuthenticate();
    }

    /**
     * Called from Rust: keeps Picacho's picture out of Recents while the app
     * lock is on (Android 13 and later); screenshots still work.
     */
    static void setRecentsPrivate(boolean on) {
        if (Build.VERSION.SDK_INT < 33) {
            return;
        }
        PicachoActivity activity = PicachoActivity.current();
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(() -> recentsScreenshot(activity, !on));
    }

    @TargetApi(33)
    private static void recentsScreenshot(PicachoActivity activity, boolean enabled) {
        activity.setRecentsScreenshotEnabled(enabled);
    }

    /** Called from Rust; answers request {@code id} later, on the UI thread. */
    static void authenticate(int id, String title, String subtitle, String cancel) {
        if (Build.VERSION.SDK_INT < 29) {
            // Rust says why, in the app's language.
            Native.answer(id, "unsupported");
            return;
        }
        PicachoActivity activity = PicachoActivity.current();
        if (activity == null) {
            Native.answer(id, "cancelled");
            return;
        }
        activity.runOnUiThread(() -> prompt(activity, id, title, subtitle, cancel));
    }

    @TargetApi(29)
    private static void prompt(
            PicachoActivity activity, int id, String title, String subtitle, String cancel) {
        try {
            BiometricPrompt.Builder builder = new BiometricPrompt.Builder(activity)
                    .setTitle(title)
                    .setSubtitle(subtitle)
                    .setConfirmationRequired(false)
                    .setNegativeButton(cancel, activity.getMainExecutor(),
                            (dialog, which) -> Native.answer(id, "cancelled"));
            if (Build.VERSION.SDK_INT >= 30) {
                builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_WEAK);
            }
            builder.build().authenticate(new CancellationSignal(), activity.getMainExecutor(),
                    new BiometricPrompt.AuthenticationCallback() {
                        @Override
                        public void onAuthenticationSucceeded(
                                BiometricPrompt.AuthenticationResult result) {
                            Native.answer(id, "ok");
                        }

                        @Override
                        public void onAuthenticationError(int code, CharSequence message) {
                            // The negative button answers through its own listener.
                            boolean cancelled = code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED
                                    || code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED;
                            Native.answer(id, cancelled
                                    ? "cancelled"
                                    : "error:" + (message != null ? message : "code " + code));
                        }
                        // A finger or face not recognized: the prompt says so
                        // and lets the person try again.
                    });
        } catch (RuntimeException e) {
            Log.w(TAG, "biometric prompt failed", e);
            Native.answer(id, "error:" + e.getMessage());
        }
    }
}
