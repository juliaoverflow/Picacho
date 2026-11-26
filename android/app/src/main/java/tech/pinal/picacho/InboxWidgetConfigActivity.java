package tech.pinal.picacho;

import android.app.Activity;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProviderInfo;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.view.View;
import android.widget.CheckBox;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import java.util.ArrayList;
import java.util.List;

/**
 * Optional configuration activity for Picacho's Inbox, Unread Mail, Packages,
 * and Receipts widgets: lets the user choose a specific account (or All
 * Accounts) and, for the Inbox widget, whether to show only unread messages.
 */
public final class InboxWidgetConfigActivity extends Activity {
    private int appWidgetId = AppWidgetManager.INVALID_APPWIDGET_ID;
    private final List<String> accountIds = new ArrayList<>();
    private RadioGroup accountGroup;
    private CheckBox unreadOnlyBox;

    @Override
    protected void attachBaseContext(Context base) {
        // Its text in Picacho's language, which may not be the phone's.
        super.attachBaseContext(Lang.wrap(base));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setResult(RESULT_CANCELED);

        Intent intent = getIntent();
        if (intent != null) {
            appWidgetId = intent.getIntExtra(
                    AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID);
        }
        if (appWidgetId == AppWidgetManager.INVALID_APPWIDGET_ID) {
            finish();
            return;
        }

        setContentView(R.layout.widget_config);
        accountGroup = findViewById(R.id.widget_config_accounts);
        unreadOnlyBox = findViewById(R.id.widget_config_unread_only);

        // Show the "unread only" toggle only when configuring the Inbox widget.
        AppWidgetManager manager = AppWidgetManager.getInstance(this);
        AppWidgetProviderInfo info = manager != null ? manager.getAppWidgetInfo(appWidgetId) : null;
        // Only one of Picacho's own widgets: the activity is exported (the
        // launcher starts it), and anyone could otherwise start it with any
        // widget id to see the accounts listed, or set a widget's choice.
        if (info == null
                || info.provider == null
                || !getPackageName().equals(info.provider.getPackageName())) {
            finish();
            return;
        }
        String providerClass =
                (info != null && info.provider != null) ? info.provider.getClassName() : "";
        boolean isInbox = InboxWidgetProvider.class.getName().equals(providerClass);
        boolean isSummary = PackagesWidgetProvider.class.getName().equals(providerClass)
                || ReceiptsWidgetProvider.class.getName().equals(providerClass);
        if (!isInbox) {
            unreadOnlyBox.setVisibility(View.GONE);
        }

        WidgetStore.Snapshot snap = WidgetStore.load(this);
        String currentAccount = WidgetStore.configuredAccount(this, appWidgetId);
        unreadOnlyBox.setChecked(WidgetStore.configuredUnreadOnly(this, appWidgetId));

        addOption(
                getString(isSummary ? R.string.widget_all_accounts : R.string.widget_all_inboxes),
                "",
                currentAccount.isEmpty());
        for (WidgetStore.Account acc : snap.accounts) {
            String label = acc.name.isEmpty() || acc.name.equals(acc.email)
                    ? acc.email
                    : getString(R.string.widget_config_account_name, acc.name, acc.email);
            addOption(label, acc.id, acc.id.equals(currentAccount));
        }

        findViewById(R.id.widget_config_cancel).setOnClickListener(v -> finish());
        findViewById(R.id.widget_config_save).setOnClickListener(v -> saveAndFinish());
    }

    private void addOption(String label, String accountId, boolean selected) {
        int index = accountIds.size();
        accountIds.add(accountId);
        RadioButton button = new RadioButton(this);
        button.setId(View.generateViewId());
        button.setTag(index);
        button.setText(label);
        button.setPadding(8, 16, 8, 16);
        accountGroup.addView(button);
        if (selected) {
            accountGroup.check(button.getId());
        }
    }

    private void saveAndFinish() {
        String chosen = "";
        int checkedId = accountGroup.getCheckedRadioButtonId();
        View checked = checkedId != View.NO_ID ? accountGroup.findViewById(checkedId) : null;
        if (checked != null && checked.getTag() instanceof Integer) {
            int index = (Integer) checked.getTag();
            if (index >= 0 && index < accountIds.size()) {
                chosen = accountIds.get(index);
            }
        }
        boolean unreadOnly = unreadOnlyBox.getVisibility() == View.VISIBLE && unreadOnlyBox.isChecked();
        WidgetStore.saveConfig(this, appWidgetId, chosen, unreadOnly);
        WidgetStore.refreshAll(this);

        Intent result = new Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId);
        setResult(RESULT_OK, result);
        finish();
    }
}
