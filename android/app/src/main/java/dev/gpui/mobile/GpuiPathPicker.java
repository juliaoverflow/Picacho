// The Java half of gpui-mobile's file prompts (its android/document_picker.rs
// calls openFiles on a worker thread and waits). Written for Picacho to the
// contract gpui-mobile's README describes; gpui-mobile doesn't ship it.
package dev.gpui.mobile;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;

/**
 * Picks files with the system's document picker (Storage Access Framework),
 * copying each into the app's cache, under its own name, so GPUI gets
 * ordinary file paths without storage permissions. A translucent activity
 * of its own receives the picker's result.
 */
public final class GpuiPathPicker extends Activity {
    private static final String TAG = "gpui";
    private static final String EXTRA_MULTIPLE = "multiple";
    private static final int PICK = 1;
    /** Copies older than this are deleted when picking again. */
    private static final long KEEP_MS = 7L * 24 * 60 * 60 * 1000;
    /**
     * The most one pick copies, per file and in all: as much as a message
     * may attach (smtp.rs), so a large selection can't fill the device.
     */
    private static final long MAX_FILE = 100L * 1024 * 1024;
    private static final long MAX_TOTAL = 100L * 1024 * 1024;
    /** Space left free on the device after copying. */
    private static final long KEEP_FREE = 200L * 1024 * 1024;

    /** One prompt at a time. */
    private static final Object prompts = new Object();
    private static volatile CountDownLatch done;
    private static volatile String[] result;

    private boolean delivered;

    /**
     * Shows the picker and waits for it (never call on the UI thread): the
     * picked files' paths, or null if none were picked.
     */
    public static String[] openFiles(Activity activity, boolean multiple) throws InterruptedException {
        synchronized (prompts) {
            CountDownLatch latch = new CountDownLatch(1);
            done = latch;
            result = null;
            activity.startActivity(new Intent(activity, GpuiPathPicker.class)
                    .putExtra(EXTRA_MULTIPLE, multiple));
            latch.await();
            return result;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if (savedInstanceState != null) {
            // Recreated while the picker is open: its result still comes here.
            return;
        }
        boolean multiple = getIntent().getBooleanExtra(EXTRA_MULTIPLE, false);
        Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*")
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
        try {
            startActivityForResult(pick, PICK);
        } catch (ActivityNotFoundException e) {
            try {
                startActivityForResult(new Intent(Intent.ACTION_GET_CONTENT)
                        .addCategory(Intent.CATEGORY_OPENABLE)
                        .setType("*/*")
                        .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple), PICK);
            } catch (ActivityNotFoundException none) {
                deliver(null);
            }
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode != PICK) {
            return;
        }
        if (resultCode != RESULT_OK || data == null) {
            deliver(null);
            return;
        }
        List<Uri> uris = new ArrayList<>();
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int i = 0; i < clip.getItemCount(); i++) {
                if (clip.getItemAt(i).getUri() != null) {
                    uris.add(clip.getItemAt(i).getUri());
                }
            }
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        // Files can be large: copy off the UI thread.
        new Thread(() -> {
            String[] paths = copy(uris);
            runOnUiThread(() -> deliver(paths));
        }, "gpui-path-picker").start();
    }

    private String[] copy(List<Uri> uris) {
        File picked = new File(getCacheDir(), "picked");
        prune(picked);
        List<String> paths = new ArrayList<>();
        ContentResolver resolver = getContentResolver();
        long total = 0;
        for (Uri uri : uris) {
            File dir = new File(picked, UUID.randomUUID().toString());
            if (!dir.mkdirs()) {
                continue;
            }
            File file = new File(dir, displayName(resolver, uri));
            try (InputStream in = resolver.openInputStream(uri);
                    OutputStream out = new FileOutputStream(file)) {
                if (in == null) {
                    continue;
                }
                byte[] buffer = new byte[64 * 1024];
                long size = 0;
                int n;
                while ((n = in.read(buffer)) > 0) {
                    size += n;
                    if (size > MAX_FILE || total + size > MAX_TOTAL
                            || picked.getUsableSpace() < KEEP_FREE + n) {
                        throw new IOException("picked files are too large to attach");
                    }
                    out.write(buffer, 0, n);
                }
                total += size;
                paths.add(file.getAbsolutePath());
            } catch (IOException | SecurityException e) {
                Log.w(TAG, "could not copy a picked file", e);
                file.delete();
                dir.delete();
            }
        }
        return paths.isEmpty() ? null : paths.toArray(new String[0]);
    }

    /** The document's name, safe as a file name. */
    private static String displayName(ContentResolver resolver, Uri uri) {
        String name = null;
        try (Cursor cursor = resolver.query(
                uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) {
                name = cursor.getString(0);
            }
        } catch (RuntimeException ignored) {
        }
        if (name == null) {
            name = uri.getLastPathSegment();
        }
        name = name == null ? "" : name.replace('/', '_').replace('\0', '_').trim();
        while (name.startsWith(".")) {
            name = name.substring(1);
        }
        return name.isEmpty() ? "file" : name;
    }

    /**
     * Deletes earlier copies past KEEP_MS. Drafts don't refer to these: the
     * composer moves each file it attaches into the draft's own files.
     */
    private static void prune(File picked) {
        File[] dirs = picked.listFiles();
        if (dirs == null) {
            return;
        }
        long now = System.currentTimeMillis();
        for (File dir : dirs) {
            if (now - dir.lastModified() > KEEP_MS) {
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

    private void deliver(String[] paths) {
        if (delivered) {
            return;
        }
        delivered = true;
        result = paths;
        CountDownLatch latch = done;
        if (latch != null) {
            latch.countDown();
        }
        if (!isFinishing()) {
            finish();
            overridePendingTransition(0, 0);
        }
    }

    @Override
    protected void onDestroy() {
        // Finished without a result: the wait ends with nothing picked.
        if (isFinishing()) {
            deliver(null);
        }
        super.onDestroy();
    }
}
