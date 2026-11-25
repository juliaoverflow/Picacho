package tech.pinal.picacho;

import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * What other apps hand Picacho: a {@code mailto:} link (Picacho as the
 * email app), an email file to read, or text, links and files shared from
 * the Share sheet (ACTION_SEND, ACTION_SEND_MULTIPLE), which start a new
 * message. Each becomes a request for Rust (src/display/launch.rs):
 * {@code mailto:…}, an email file's path, or {@code attach:} and a path.
 * Shared files are copied into the app's cache, as the apps sharing them
 * grant their content only for a while.
 */
final class Shared {
    private Shared() {}

    /** The longest shared text kept, as Rust keeps requests. */
    private static final int MAX_TEXT = 6000;

    static List<String> requests(Context context, Intent intent) {
        List<String> out = new ArrayList<>();
        if (intent == null || intent.getAction() == null) {
            return out;
        }
        String action = intent.getAction();
        Uri data = intent.getData();
        if ((Intent.ACTION_VIEW.equals(action) || Intent.ACTION_SENDTO.equals(action))
                && data != null && "mailto".equalsIgnoreCase(data.getScheme())) {
            out.add(data.toString());
            return out;
        }
        if (Intent.ACTION_VIEW.equals(action) && data != null) {
            String mime = intent.getType();
            String fallback = "application/vnd.ms-outlook".equalsIgnoreCase(mime)
                    ? "message.msg"
                    : "message.eml";
            File file = copy(context, data, fallback);
            if (file != null) {
                out.add(file.getAbsolutePath());
            }
            return out;
        }
        if (Intent.ACTION_SEND.equals(action) || Intent.ACTION_SEND_MULTIPLE.equals(action)) {
            StringBuilder mailto = new StringBuilder("mailto:");
            String[] to = intent.getStringArrayExtra(Intent.EXTRA_EMAIL);
            if (to != null) {
                mailto.append(Uri.encode(String.join(",", to), "@,"));
            }
            String sep = "?";
            String subject = intent.getStringExtra(Intent.EXTRA_SUBJECT);
            if (subject != null && !subject.isEmpty()) {
                mailto.append(sep).append("subject=").append(Uri.encode(subject));
                sep = "&";
            }
            CharSequence text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT);
            if (text != null && text.length() > 0) {
                String body = text.toString();
                if (body.length() > MAX_TEXT) {
                    body = body.substring(0, MAX_TEXT);
                }
                mailto.append(sep).append("body=").append(Uri.encode(body));
            }
            out.add(mailto.toString());
            List<Uri> streams = new ArrayList<>();
            if (Intent.ACTION_SEND.equals(action)) {
                Uri stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
                if (stream != null) {
                    streams.add(stream);
                }
            } else {
                ArrayList<Uri> many = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
                if (many != null) {
                    streams.addAll(many);
                }
            }
            for (Uri uri : streams) {
                File file = copy(context, uri, "attachment");
                if (file != null) {
                    out.add("attach:" + file.getAbsolutePath());
                }
            }
        }
        return out;
    }

    /** The content at {@code uri}, copied into the cache under its own name. */
    private static File copy(Context context, Uri uri, String fallback) {
        ContentResolver resolver = context.getContentResolver();
        String name = fallback;
        try (Cursor c = resolver.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) {
                name = c.getString(0);
            }
        } catch (RuntimeException e) {
            // Not every provider answers; the fallback name does.
        }
        name = name.replace('/', '_').replace('\\', '_').replace("\0", "_");
        // An email file keeps its extension (.eml or .msg), which Rust goes by.
        String lower = name.toLowerCase(java.util.Locale.ROOT);
        if ((fallback.endsWith(".eml") || fallback.endsWith(".msg"))
                && !lower.endsWith(".eml") && !lower.endsWith(".msg")) {
            name = name + (fallback.endsWith(".msg") ? ".msg" : ".eml");
        }
        File dir = new File(context.getCacheDir(), "shared/" + System.nanoTime());
        if (!dir.mkdirs()) {
            return null;
        }
        File file = new File(dir, name);
        try (InputStream in = resolver.openInputStream(uri);
                OutputStream os = new FileOutputStream(file)) {
            if (in == null) {
                return null;
            }
            byte[] buffer = new byte[64 * 1024];
            int n;
            while ((n = in.read(buffer)) > 0) {
                os.write(buffer, 0, n);
            }
            return file;
        } catch (Exception e) {
            return null;
        }
    }
}
