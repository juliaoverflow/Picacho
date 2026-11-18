package tech.pinal.picacho;

import android.app.Notification;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import android.util.Log;

/**
 * Runs while a browser sign-in is open ({@link SignIn}): a foreground
 * service, so Android doesn't freeze Picacho while the browser is in front.
 * The provider's last step is a request to Picacho's loopback listener,
 * which a frozen process wouldn't answer.
 */
public final class SignInService extends Service {
    private static final String TAG = "picacho";

    static void start(Context context) {
        try {
            context.startForegroundService(new Intent(context, SignInService.class));
        } catch (RuntimeException e) {
            // Refused from the background (Android 12 and later): the
            // sign-in still works while Picacho isn't frozen.
            Log.w(TAG, "could not keep Picacho running for the sign-in", e);
        }
    }

    static void stop(Context context) {
        context.stopService(new Intent(context, SignInService.class));
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Notification notification = Notifier.signingIn(this);
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(
                    Notifier.SIGN_IN_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(Notifier.SIGN_IN_ID, notification);
        }
        // A process restarted without the sign-in has nothing to wait for.
        return START_NOT_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
