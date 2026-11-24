package tech.pinal.picacho;

import android.content.Context;

/**
 * The Rust side (libpicacho.so, src/android.rs). The activity and the
 * service each load it before calling in; loading it twice in a process
 * gives the same library, whose state they share.
 */
final class Native {
    static {
        System.loadLibrary("picacho");
    }

    private Native() {}

    /** Hands over the application's context; first thing in each component. */
    static native void attach(Context context);

    /** The process's first activity is about to start its window. */
    static native void beforeActivity();

    /** {@link MailService} started; {@code dataDir} holds the mail store. */
    static native void serviceStarted(String dataDir);

    /** {@link MailService} ended. */
    static native void serviceStopped();

    /** The process is about to be ended: stop checking and save what's pending. */
    static native void shutdown();

    /**
     * Back, from PicachoActivity's back callback: {@code phase} 0 a swipe
     * started, 1 it moved ({@code progress} 0 to 1), 2 go back, 3 the swipe
     * was let go of without going back.
     */
    static native void back(int phase, float progress);

    /**
     * The system bars', display cutout's and keyboard's insets, in points,
     * from PicachoActivity's window insets.
     */
    static native void insets(float top, float bottom, float left, float right, float keyboard);

    /** Whether the system is in dark mode, from PicachoActivity's configuration. */
    static native void nightMode(boolean dark);

    /** A notification was tapped: {@code target} is where it goes (see Notifier). */
    static native void open(String target);

    /**
     * A new-mail notification's button was pressed (Mark as Read, Archive,
     * Delete): {@code target} names the messages and the button (see
     * Notifier).
     */
    static native void quick(String target);

    /**
     * Another app handed Picacho something to open (see {@link Shared}): a
     * mailto: link, an email file's path, or "attach:" and a shared file's.
     */
    static native void launch(String request);

    /**
     * An answer to a request Rust made ({@link VerifyActivity}, {@link Passkeys}):
     * {@code value} is the answer, or null when there is nothing more.
     */
    static native void answer(int id, String value);
}
