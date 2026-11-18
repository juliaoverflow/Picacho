package tech.pinal.picacho;

import android.Manifest;
import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.drawable.Icon;
import android.os.Build;
import java.util.Arrays;

/** Picacho's notifications: new mail (from Rust) and the ongoing service one. */
final class Notifier {
    /** The service's ongoing notification. Rust's ids are 1000 and up. */
    static final int ONGOING_ID = 1;
    /** {@link SignInService}'s, while a browser sign-in is open. */
    static final int SIGN_IN_ID = 2;
    private static final String MAIL_CHANNEL = "mail";
    private static final String BACKGROUND_CHANNEL = "background";
    private static final String SIGN_IN_CHANNEL = "sign-in";
    private static final int PERMISSION_REQUEST = 1;
    /** Where a tapped notification goes (Rust's notify::Target, as JSON). */
    static final String EXTRA_OPEN = "tech.pinal.picacho.OPEN";

    private Notifier() {}

    private static NotificationManager manager(Context context) {
        NotificationManager manager = context.getSystemService(NotificationManager.class);
        // Creating channels that exist changes nothing.
        NotificationChannel mail = new NotificationChannel(
                MAIL_CHANNEL, "New mail", NotificationManager.IMPORTANCE_HIGH);
        mail.setDescription("New messages in your inboxes, and notices from your mail provider.");
        NotificationChannel background = new NotificationChannel(
                BACKGROUND_CHANNEL, "Checking for mail", NotificationManager.IMPORTANCE_MIN);
        background.setDescription("Shown while Picacho checks for new mail in the background.");
        background.setShowBadge(false);
        NotificationChannel signIn = new NotificationChannel(
                SIGN_IN_CHANNEL, "Signing in", NotificationManager.IMPORTANCE_LOW);
        signIn.setDescription("Shown while you sign in to an account in the browser.");
        signIn.setShowBadge(false);
        manager.createNotificationChannels(Arrays.asList(mail, background, signIn));
        return manager;
    }

    /**
     * Called from Rust. A notification with the same id is replaced. Tapping
     * it opens Picacho at {@code target} (empty for none; see
     * {@link #targetOf}).
     */
    public static void show(
            Context context, int id, String title, String text, String subText, String target) {
        Notification notification = new Notification.Builder(context, MAIL_CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(title)
                .setContentText(text)
                .setSubText(subText)
                .setStyle(new Notification.BigTextStyle().bigText(text))
                .setCategory(Notification.CATEGORY_EMAIL)
                .setContentIntent(open(context, id, target))
                .setAutoCancel(true)
                .build();
        try {
            manager(context).notify(id, notification);
        } catch (SecurityException e) {
            // Notifications aren't allowed; nothing to show them with.
        }
    }

    static Notification ongoing(Context context) {
        PendingIntent stop = PendingIntent.getService(
                context,
                0,
                new Intent(context, MailService.class).setAction(MailService.ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification.Action stopAction = new Notification.Action.Builder(
                        Icon.createWithResource(context, R.drawable.ic_notification), "Stop", stop)
                .build();
        manager(context);
        return new Notification.Builder(context, BACKGROUND_CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("Checking for new mail")
                .setContentText("Picacho lets you know when new mail arrives.")
                .setCategory(Notification.CATEGORY_SERVICE)
                .setContentIntent(openApp(context))
                .setOngoing(true)
                .setShowWhen(false)
                .addAction(stopAction)
                .build();
    }

    static Notification signingIn(Context context) {
        manager(context);
        return new Notification.Builder(context, SIGN_IN_CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("Signing in")
                .setContentText("Finish signing in in the browser, then come back to Picacho.")
                .setCategory(Notification.CATEGORY_PROGRESS)
                .setContentIntent(openApp(context))
                .setOngoing(true)
                .setShowWhen(false)
                .build();
    }

    private static PendingIntent openApp(Context context) {
        return open(context, 0, null);
    }

    /**
     * Opens Picacho as its launcher icon does (the same component, so the
     * same task comes forward), carrying {@code target} when there is one.
     * Each notification has its own request code: one PendingIntent per
     * notification, so each keeps its own target.
     */
    private static PendingIntent open(Context context, int requestCode, String target) {
        Intent open = new Intent(Intent.ACTION_MAIN)
                .setClassName(context, "android.app.NativeActivity")
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (target != null && !target.isEmpty()) {
            open.putExtra(EXTRA_OPEN, target);
        }
        return PendingIntent.getActivity(
                context,
                requestCode,
                open,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    /**
     * The target a tapped notification's intent carries, or null. An intent
     * relaunched from Recents carries what it first did; that isn't a tap.
     */
    static String targetOf(Intent intent) {
        if (intent == null
                || (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0) {
            return null;
        }
        String target = intent.getStringExtra(EXTRA_OPEN);
        return target == null || target.isEmpty() ? null : target;
    }

    /** Asks for permission to notify (Android 13 and later) if not granted. */
    static void requestPermission(Activity activity) {
        if (Build.VERSION.SDK_INT >= 33
                && activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED) {
            activity.requestPermissions(
                    new String[] {Manifest.permission.POST_NOTIFICATIONS}, PERMISSION_REQUEST);
        }
    }
}
