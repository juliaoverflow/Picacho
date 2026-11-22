package tech.pinal.picacho;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;

/**
 * Android's clipboard, for Rust (src/android.rs): GPUI's Android platform
 * keeps a clipboard of its own, so Picacho copies between the two.
 */
final class Clipboard {
    private Clipboard() {}

    /** The clipboard's text, or null. Android only lets the focused app read it. */
    public static String read(Context context) {
        try {
            ClipboardManager manager = context.getSystemService(ClipboardManager.class);
            ClipData clip = manager == null ? null : manager.getPrimaryClip();
            if (clip == null || clip.getItemCount() == 0) {
                return null;
            }
            CharSequence text = clip.getItemAt(0).coerceToText(context);
            return text == null ? null : text.toString();
        } catch (RuntimeException e) {
            return null;
        }
    }

    /** The clipboard's HTML (what was copied, with its formatting), or null. */
    public static String readHtml(Context context) {
        try {
            ClipboardManager manager = context.getSystemService(ClipboardManager.class);
            ClipData clip = manager == null ? null : manager.getPrimaryClip();
            if (clip == null || clip.getItemCount() == 0) {
                return null;
            }
            return clip.getItemAt(0).getHtmlText();
        } catch (RuntimeException e) {
            return null;
        }
    }

    /**
     * Whether the system asks apps to reduce motion: Settings → Accessibility
     * → Remove animations (which sets the animator duration scale to 0).
     */
    public static boolean reduceMotion(Context context) {
        try {
            float scale = android.provider.Settings.Global.getFloat(
                    context.getContentResolver(),
                    android.provider.Settings.Global.ANIMATOR_DURATION_SCALE,
                    1f);
            return scale == 0f;
        } catch (RuntimeException e) {
            return false;
        }
    }

    /**
     * Puts {@code text} on the clipboard. False when Android refused it:
     * text past about a megabyte (a whole message with its attachments)
     * is more than it hands between apps.
     */
    public static boolean write(Context context, String text) {
        return put(context, text, false);
    }

    /**
     * Puts a secret (a sign-in code) on the clipboard, marked sensitive so
     * Android 13 and later don't show it in the copied-text preview, and
     * keyboards with clipboard history are asked not to keep it.
     */
    public static boolean writeSensitive(Context context, String text) {
        return put(context, text, true);
    }

    private static boolean put(Context context, String text, boolean sensitive) {
        try {
            ClipboardManager manager = context.getSystemService(ClipboardManager.class);
            if (manager == null) {
                return false;
            }
            ClipData clip = ClipData.newPlainText("Picacho", text);
            if (sensitive) {
                android.os.PersistableBundle extras = new android.os.PersistableBundle();
                // ClipDescription.EXTRA_IS_SENSITIVE (API 33), by its value,
                // which earlier versions' keyboards also read.
                extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                clip.getDescription().setExtras(extras);
            }
            manager.setPrimaryClip(clip);
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }
}
