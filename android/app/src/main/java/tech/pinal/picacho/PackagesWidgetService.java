package tech.pinal.picacho;

import android.appwidget.AppWidgetManager;
import android.content.Context;
import android.content.Intent;
import android.view.View;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;
import java.util.ArrayList;
import java.util.List;

/** Supplies the recent package rows to {@link PackagesWidgetProvider}. */
public final class PackagesWidgetService extends RemoteViewsService {
    @Override
    public RemoteViewsFactory onGetViewFactory(Intent intent) {
        int appWidgetId = intent.getIntExtra(
                AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID);
        return new Factory(getApplicationContext(), appWidgetId);
    }

    private static final class Factory implements RemoteViewsFactory {
        private static final int[] STEP_IDS = {
            R.id.widget_pkg_step_0,
            R.id.widget_pkg_step_1,
            R.id.widget_pkg_step_2,
            R.id.widget_pkg_step_3,
            R.id.widget_pkg_step_4,
        };
        private final Context context;
        private final int appWidgetId;
        private final List<WidgetStore.TrackedPackage> items = new ArrayList<>();
        private String accountId = "";
        private boolean showAccountLabel;
        private int accent;

        Factory(Context context, int appWidgetId) {
            this.context = context;
            this.appWidgetId = appWidgetId;
        }

        @Override
        public void onCreate() {}

        @Override
        public void onDataSetChanged() {
            WidgetStore.Snapshot snap = WidgetStore.load(context);
            accountId = WidgetStore.configuredAccount(context, appWidgetId);
            showAccountLabel = accountId.isEmpty() && snap.accounts.size() > 1;
            accent = WidgetStore.accentColor(context, snap);
            items.clear();
            // Every one the snapshot holds: the list shows as many as the widget's
            // size fits, and scrolls to the rest.
            items.addAll(snap.filteredPackages(accountId, Integer.MAX_VALUE));
        }

        @Override
        public void onDestroy() {
            items.clear();
        }

        @Override
        public int getCount() {
            return items.size();
        }

        @Override
        public RemoteViews getViewAt(int position) {
            if (position < 0 || position >= items.size()) {
                return null;
            }
            WidgetStore.TrackedPackage p = items.get(position);
            RemoteViews row = new RemoteViews(context.getPackageName(), R.layout.widget_packages_item);

            int muted = context.getColor(R.color.widget_text_muted);
            int divider = context.getColor(R.color.widget_divider);
            int fillColor = p.delivered ? muted : accent;

            row.setInt(R.id.widget_pkg_item_icon, "setColorFilter", fillColor);
            row.setTextViewText(R.id.widget_pkg_item_title, p.title.isEmpty() ? "Package" : p.title);
            if (p.number.isEmpty()) {
                row.setViewVisibility(R.id.widget_pkg_item_number, View.GONE);
            } else {
                row.setViewVisibility(R.id.widget_pkg_item_number, View.VISIBLE);
                row.setTextViewText(R.id.widget_pkg_item_number, p.number);
            }

            StringBuilder detail = new StringBuilder();
            if (!p.status.isEmpty()) {
                detail.append(p.status);
            }
            if (!p.carrier.isEmpty()) {
                if (detail.length() > 0) detail.append(" · ");
                detail.append(p.carrier);
            }
            if (!p.expected.isEmpty()) {
                if (detail.length() > 0) detail.append(" · ");
                detail.append("Expected ").append(p.expected);
            } else {
                String date = WidgetStore.formatMessageDate(context, p.updated);
                if (!date.isEmpty()) {
                    if (detail.length() > 0) detail.append(" · ");
                    detail.append(date);
                }
            }
            if (showAccountLabel && !p.accountLabel.isEmpty()) {
                if (detail.length() > 0) detail.append(" · ");
                detail.append(p.accountLabel);
            }
            row.setTextViewText(R.id.widget_pkg_item_detail, detail.toString());

            int stage = Math.max(0, Math.min(4, p.stage));
            for (int step = 0; step < STEP_IDS.length; step++) {
                row.setInt(STEP_IDS[step], "setColorFilter", step <= stage ? fillColor : divider);
            }

            String uri = (!p.accountId.isEmpty() && !p.mailId.isEmpty())
                    ? WidgetStore.messageUri(p.accountId, p.mailId)
                    : WidgetStore.packagesUri(accountId);
            row.setOnClickFillInIntent(R.id.widget_pkg_item_root, WidgetStore.fillInIntent(context, uri));
            return row;
        }

        @Override
        public RemoteViews getLoadingView() {
            return null;
        }

        @Override
        public int getViewTypeCount() {
            return 1;
        }

        @Override
        public long getItemId(int position) {
            if (position < 0 || position >= items.size()) {
                return position;
            }
            return items.get(position).key.hashCode();
        }

        @Override
        public boolean hasStableIds() {
            return true;
        }
    }
}
