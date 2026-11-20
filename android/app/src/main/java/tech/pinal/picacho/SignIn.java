package tech.pinal.picacho;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;

/**
 * Browser sign-ins (Gmail, Outlook, Pinal), for Rust (sync/oauth.rs). The
 * provider's page opens in a Custom Tab over Picacho where the browser has
 * them, else in the browser. Its last step goes to Picacho's loopback
 * listener, so while it is open {@link SignInService} keeps the process from
 * being frozen; that page then brings Picacho back with the app's own scheme
 * (see PicachoActivity.onNewIntent).
 */
final class SignIn {
    private static final String TAG = "picacho";
    /** The Custom Tabs protocol's extras, without the androidx library. */
    private static final String EXTRA_SESSION = "android.support.customtabs.extra.SESSION";
    private static final String EXTRA_TITLE = "android.support.customtabs.extra.TITLE_VISIBILITY";

    private SignIn() {}

    /** Called from Rust: opens {@code url}; false if nothing could show it. */
    public static boolean open(Context context, String url) {
        Uri uri = Uri.parse(url);
        if (!"https".equals(uri.getScheme())) {
            return false;
        }
        SignInService.start(context);
        Intent view = new Intent(Intent.ACTION_VIEW, uri)
                .addCategory(Intent.CATEGORY_BROWSABLE);
        // A Custom Tab: an extras bundle with a (null) session binder.
        Bundle session = new Bundle();
        session.putBinder(EXTRA_SESSION, null);
        view.putExtras(session);
        view.putExtra(EXTRA_TITLE, 1);
        // Over Picacho, in its task, when its window is there: going back to
        // Picacho then closes the tab.
        Activity activity = PicachoActivity.current();
        try {
            if (activity != null && !activity.isFinishing()) {
                activity.startActivity(view);
            } else {
                context.startActivity(view.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            }
            return true;
        } catch (ActivityNotFoundException | SecurityException e) {
            Log.w(TAG, "no browser for the sign-in", e);
            SignInService.stop(context);
            return false;
        }
    }

    /** Called from Rust once the sign-in is over, however it ended. */
    public static void ended(Context context) {
        SignInService.stop(context);
    }
}
