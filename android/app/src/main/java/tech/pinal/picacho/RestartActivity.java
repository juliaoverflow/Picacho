package tech.pinal.picacho;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.os.Process;

/**
 * Restarts Picacho's process (see {@link PicachoActivity}): runs in a
 * process of its own, ends the app's, opens Picacho again in a new one, and
 * ends its own.
 */
public final class RestartActivity extends Activity {
    static final String EXTRA_PID = "tech.pinal.picacho.PID";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        int pid = getIntent().getIntExtra(EXTRA_PID, -1);
        if (pid > 0 && pid != Process.myPid()) {
            Process.killProcess(pid);
        }
        Intent open = new Intent(this, PicachoActivity.class)
                .setAction(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
        startActivity(open);
        finish();
        Process.killProcess(Process.myPid());
    }
}
