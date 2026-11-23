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
        // Web and email links only, whatever asked: an intent:, file: or
        // content: address could start any app, or hand over a file.
        Uri uri = Uri.parse(url);
        String scheme = uri.getScheme() == null ? "" : uri.getScheme().toLowerCase(java.util.Locale.ROOT);
        if (!scheme.equals("https") && !scheme.equals("http") && !scheme.equals("mailto")) {
            return false;
        }
        Intent view = new Intent(Intent.ACTION_VIEW, uri)
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
