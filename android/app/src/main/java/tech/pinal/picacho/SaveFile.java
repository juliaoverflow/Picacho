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
import java.util.concurrent.TimeUnit;

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
    /** The save this activity answers. */
    private CountDownLatch mine;
    /** Set once the activity starts: until then, the save may never begin. */
    private static volatile boolean started;

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
            started = false;
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
            // Android may not start an activity from the background: then
            // nothing would ever answer, and every later save would wait
            // behind this one. Once it has started, the person may take as
            // long as they like choosing where.
            if (!latch.await(10, TimeUnit.SECONDS) && !started) {
                // Should it start after all, it answers no one.
                done = null;
                return FAILED;
            }
            latch.await();
            return result;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        started = true;
        // Recreated (the screen turned) while its picker was open: the same
        // save, still waiting, which nothing has answered yet.
        mine = done;
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
        CountDownLatch latch = done;
        if (latch != null && latch == mine) {
            result = outcome;
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
