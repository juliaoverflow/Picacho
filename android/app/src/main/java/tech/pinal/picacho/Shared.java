package tech.pinal.picacho;

import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.provider.OpenableColumns;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
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
    /**
     * The most one share copies, per file and in all (as much as a message
     * may attach, as GpuiPathPicker's picks), and the most files: a share of
     * an endless stream ({@code /dev/zero}, a pipe that never closes) or of
     * thousands of files can't fill the device or run for ever.
     */
    private static final long MAX_FILE = 100L * 1024 * 1024;
    private static final long MAX_TOTAL = 100L * 1024 * 1024;
    private static final int MAX_FILES = 64;
    /** Space left free on the device after copying. */
    private static final long KEEP_FREE = 200L * 1024 * 1024;
    /** Copies older than this are deleted when something is shared again. */
    private static final long KEEP_MS = 7L * 24 * 60 * 60 * 1000;

    /**
     * The requests {@code intent} makes. Copies shared files, which may be
     * large or slow (another app's provider): never call it on the UI
     * thread (see {@link PicachoActivity}).
     */
    static List<String> requests(Context context, Intent intent) {
        List<String> out = new ArrayList<>();
        try {
            if (intent == null || intent.getAction() == null) {
                return out;
            }
            return requestsOf(context, intent, out);
        } catch (RuntimeException e) {
            // A malformed intent from another app (a parcelable that isn't a
            // Uri, a bad parcel): what was read so far, nothing more.
            return out;
        }
    }

    private static List<String> requestsOf(Context context, Intent intent, List<String> out) {
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
            File file = copy(context, data, fallback, new long[] {MAX_TOTAL});
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
                Object stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
                if (stream instanceof Uri) {
                    streams.add((Uri) stream);
                }
            } else {
                ArrayList<?> many = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
                if (many != null) {
                    for (Object item : many) {
                        if (item instanceof Uri && streams.size() < MAX_FILES) {
                            streams.add((Uri) item);
                        }
                    }
                }
            }
            long[] budget = {MAX_TOTAL};
            for (Uri uri : streams) {
                File file = copy(context, uri, "attachment", budget);
                if (file != null) {
                    out.add("attach:" + file.getAbsolutePath());
                }
            }
        }
        return out;
    }

    /**
     * Whether {@code uri} may be read on another app's behalf: only content
     * and file URIs, and none of Picacho's own. Reading is done with
     * Picacho's permissions, so an app that can't read Picacho's private
     * files could otherwise name one (its database, its copies of opened
     * attachments) and have it attached to a message it addressed.
     */
    private static boolean readable(Context context, Uri uri) {
        String scheme = uri.getScheme();
        if ("content".equalsIgnoreCase(scheme)) {
            String authority = uri.getAuthority();
            String own = context.getPackageName();
            return authority != null
                    && !authority.equals(own)
                    && !authority.startsWith(own + ".");
        }
        return "file".equalsIgnoreCase(scheme);
    }

    /**
     * Opens {@code uri} for reading, refusing a file owned by Picacho itself
     * (its user id): whatever path or provider leads to it, a file in its
     * private storage is Picacho's own. A file URI must be such a file; a
     * provider that only streams (a pipe) is another app's data. A file URI
     * must name a regular file: not a device ({@code /dev/zero}, owned by
     * root, reads without end), a pipe or a socket.
     */
    private static InputStream open(ContentResolver resolver, Uri uri) throws IOException {
        ParcelFileDescriptor pfd;
        boolean file = "file".equalsIgnoreCase(uri.getScheme());
        try {
            pfd = resolver.openFileDescriptor(uri, "r");
        } catch (IOException | RuntimeException e) {
            if (file) {
                throw new IOException(e);
            }
            return resolver.openInputStream(uri);
        }
        if (pfd == null) {
            return null;
        }
        try {
            StructStat stat = Os.fstat(pfd.getFileDescriptor());
            if (stat.st_uid == Process.myUid()) {
                throw new IOException("Picacho's own file");
            }
            if (file && !OsConstants.S_ISREG(stat.st_mode)) {
                throw new IOException("not a regular file");
            }
        } catch (ErrnoException | IOException e) {
            pfd.close();
            throw e instanceof IOException ? (IOException) e : new IOException(e);
        }
        return new ParcelFileDescriptor.AutoCloseInputStream(pfd);
    }

    /**
     * The content at {@code uri}, copied into the cache under its own name,
     * taking its size from {@code budget[0]} (none past it, nor past
     * {@code MAX_FILE} or {@code KEEP_FREE}).
     */
    private static File copy(Context context, Uri uri, String fallback, long[] budget) {
        if (uri == null || !readable(context, uri)) {
            return null;
        }
        prune(new File(context.getCacheDir(), "shared"));
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
        if (name.isEmpty() || name.equals(".") || name.equals("..")) {
            name = fallback;
        }
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
        try (InputStream in = open(resolver, uri)) {
            if (in == null) {
                dir.delete();
                return null;
            }
            budget[0] -= copyTo(in, file, Math.min(MAX_FILE, budget[0]));
            return file;
        } catch (Exception e) {
            file.delete();
            dir.delete();
            return null;
        }
    }

    /** Copies {@code in} to {@code file}: at most {@code limit} bytes, or none. */
    private static long copyTo(InputStream in, File file, long limit) throws IOException {
        long size = 0;
        try (OutputStream os = new FileOutputStream(file)) {
            byte[] buffer = new byte[64 * 1024];
            int n;
            while ((n = in.read(buffer)) > 0) {
                size += n;
                if (size > limit || file.getUsableSpace() < KEEP_FREE + n) {
                    throw new IOException("the shared file is too large");
                }
                os.write(buffer, 0, n);
            }
        }
        return size;
    }

    /** Deletes earlier shares' copies older than {@code KEEP_MS}. */
    private static void prune(File shared) {
        File[] dirs = shared.listFiles();
        if (dirs == null) {
            return;
        }
        long cutoff = System.currentTimeMillis() - KEEP_MS;
        for (File dir : dirs) {
            if (dir.lastModified() < cutoff) {
                File[] files = dir.listFiles();
                if (files != null) {
                    for (File f : files) {
                        f.delete();
                    }
                }
                dir.delete();
            }
        }
    }
}
