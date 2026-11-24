package tech.pinal.picacho;

import android.app.Notification;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.util.Log;
import java.io.File;

/**
 * Checking for mail in the background (Settings → General). A foreground
 * service: it keeps Picacho's process alive and unfrozen, with an ongoing
 * notification, so the window keeps checking while it's not on screen and
 * the Rust background monitor checks once the activity is gone. Started
 * again by Android after it reclaims the process, and at boot.
 */
public final class MailService extends Service {
    private static final String TAG = "picacho";
    /** The notification's Stop action: stop until Picacho is opened again. */
    static final String ACTION_STOP = "tech.pinal.picacho.action.STOP";
    /** A new-mail notification's button: Mark as Read, Archive or Delete. */
    static final String ACTION_QUICK = "tech.pinal.picacho.action.QUICK";
    /**
     * How long the service stays after a button started it while checking in
     * the background is off: long enough for the change to reach the server.
     */
    private static final long QUICK_STAY_MS = 60_000;
    /** The setting, as last told by the app, for starting at boot. */
    private static final String PREFERENCES = "background";
    private static final String ENABLED = "enabled";

    /** Called from Rust when the window starts and when the setting changes. */
    public static void setEnabled(Context context, boolean enabled) {
        preferences(context).edit().putBoolean(ENABLED, enabled).apply();
        if (enabled) {
            start(context);
        } else {
            context.stopService(new Intent(context, MailService.class));
        }
    }

    /** Whether checking in the background is on. Off until the app says. */
    static boolean enabled(Context context) {
        return preferences(context).getBoolean(ENABLED, false);
    }

    static void start(Context context) {
        try {
            context.startForegroundService(new Intent(context, MailService.class));
        } catch (RuntimeException e) {
            // Android 12 and later refuse to start one from the background.
            Log.w(TAG, "could not start checking mail in the background", e);
        }
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        goForeground();
        Native.attach(getApplicationContext());
        Native.serviceStarted(new File(getFilesDir(), "picacho").getPath());
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stopSelf();
            return START_NOT_STICKY;
        }
        // Each startForegroundService asks for it again.
        goForeground();
        if (intent != null && ACTION_QUICK.equals(intent.getAction())) {
            Notifier.cancel(this, intent.getIntExtra(Notifier.EXTRA_NOTIFICATION, 0));
            // Only Picacho's own notifications (see Notifier.EXTRA_KEY).
            String target = Notifier.targetOf(this, intent);
            if (target != null) {
                Native.quick(target);
            }
            if (!enabled(this)) {
                // Started only for this: gone again once it's done.
                new Handler(Looper.getMainLooper()).postDelayed(this::stopSelf, QUICK_STAY_MS);
                return START_NOT_STICKY;
            }
        }
        return START_STICKY;
    }

    private void goForeground() {
        Notification notification = Notifier.ongoing(this);
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(
                    Notifier.ONGOING_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(Notifier.ONGOING_ID, notification);
        }
    }

    @Override
    public void onDestroy() {
        Native.serviceStopped();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
