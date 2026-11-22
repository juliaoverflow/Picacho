package tech.pinal.picacho;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.concurrent.CountDownLatch;

/**
 * "Save as" for Rust (src/android.rs, save_as): GPUI's Android platform has
 * no save dialog, so the system's document picker (Storage Access Framework)
 * asks where, and the file Rust wrote to the app's cache is copied there.
 * A translucent activity of its own receives the picker's result.
 */
public final class SaveFile extends Activity {
    private static final String TAG = "picacho";
    private static final String EXTRA_NAME = "name";
    private static final String EXTRA_MIME = "mime";
    private static final String EXTRA_SOURCE = "source";
    private static final int CREATE = 1;

    /** Results for Rust. */
    static final int SAVED = 0;
    static final int CANCELLED = 1;
    static final int FAILED = 2;

    /** One prompt at a time. */
    private static final Object prompts = new Object();
    private static volatile CountDownLatch done;
    private static volatile int result;

    private boolean delivered;
    private String source;

    /**
     * Called from Rust, never on the UI thread: asks where to save
     * {@code source} as {@code name}, copies it there and waits. SAVED,
     * CANCELLED or FAILED.
     */
    public static int save(Context context, String source, String name, String mime)
            throws InterruptedException {
        synchronized (prompts) {
            CountDownLatch latch = new CountDownLatch(1);
            done = latch;
            result = FAILED;
            Activity activity = PicachoActivity.current();
            Context from = activity != null ? activity : context;
            Intent intent = new Intent(from, SaveFile.class)
                    .putExtra(EXTRA_SOURCE, source)
                    .putExtra(EXTRA_NAME, name)
                    .putExtra(EXTRA_MIME, mime);
            if (activity == null) {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            }
            from.startActivity(intent);
            latch.await();
            return result;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        source = getIntent().getStringExtra(EXTRA_SOURCE);
        if (savedInstanceState != null) {
            // Recreated while the picker is open: its result still comes here.
            return;
        }
        String mime = getIntent().getStringExtra(EXTRA_MIME);
        Intent create = new Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mime == null ? "application/octet-stream" : mime)
                .putExtra(Intent.EXTRA_TITLE, getIntent().getStringExtra(EXTRA_NAME));
        try {
            startActivityForResult(create, CREATE);
        } catch (ActivityNotFoundException e) {
            deliver(FAILED);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode != CREATE) {
            return;
        }
        Uri uri = data == null ? null : data.getData();
        if (resultCode != RESULT_OK || uri == null || source == null) {
            deliver(CANCELLED);
            return;
        }
        // Files can be large: copy off the UI thread.
        new Thread(() -> {
            int copied = copy(uri) ? SAVED : FAILED;
            runOnUiThread(() -> deliver(copied));
        }, "picacho-save").start();
    }

    private boolean copy(Uri uri) {
        try (InputStream in = new FileInputStream(new File(source));
                OutputStream out = getContentResolver().openOutputStream(uri, "wt")) {
            if (out == null) {
                return false;
            }
            byte[] buffer = new byte[64 * 1024];
            int n;
            while ((n = in.read(buffer)) > 0) {
                out.write(buffer, 0, n);
            }
            return true;
        } catch (IOException | RuntimeException e) {
            Log.w(TAG, "could not save a file", e);
            return false;
        }
    }

    private void deliver(int outcome) {
        if (delivered) {
            return;
        }
        delivered = true;
        result = outcome;
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
        // Finished without a result: nothing was saved.
        if (isFinishing()) {
            deliver(CANCELLED);
        }
        super.onDestroy();
    }
}
