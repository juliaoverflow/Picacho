package tech.pinal.picacho;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Starts checking for mail in the background after boot and app updates, if on. */
public final class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        if (Intent.ACTION_BOOT_COMPLETED.equals(action)
                || Intent.ACTION_MY_PACKAGE_REPLACED.equals(action)) {
            WidgetStore.refreshAll(context);
            if (MailService.enabled(context)) {
                MailService.start(context);
            }
        }
    }
}
