package tech.pinal.picacho;

import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;

/** Opens links from Rust (src/android.rs, open_url) in the app that handles them. */
final class Browser {
    private Browser() {}

    /**
     * Called from Rust: opens {@code url} (a web or email link, checked
     * there) in the browser or mail app. Returns false when nothing can.
     */
    public static boolean open(Context context, String url) {
        Intent view = new Intent(Intent.ACTION_VIEW, Uri.parse(url))
                .addCategory(Intent.CATEGORY_BROWSABLE)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            context.startActivity(view);
            return true;
        } catch (ActivityNotFoundException | SecurityException e) {
            return false;
        }
    }
}
