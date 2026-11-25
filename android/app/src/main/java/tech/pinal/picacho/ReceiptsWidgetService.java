package tech.pinal.picacho;

import android.appwidget.AppWidgetManager;
import android.content.Context;
import android.content.Intent;
import android.view.View;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;
import java.util.ArrayList;
import java.util.List;

/** Supplies the recent receipt rows to {@link ReceiptsWidgetProvider}. */
public final class ReceiptsWidgetService extends RemoteViewsService {
    @Override
    public RemoteViewsFactory onGetViewFactory(Intent intent) {
        int appWidgetId = intent.getIntExtra(
                AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID);
        return new Factory(getApplicationContext(), appWidgetId);
    }

    private static final class Factory implements RemoteViewsFactory {
        private final Context context;
        private final int appWidgetId;
        private final List<WidgetStore.ReceiptItem> items = new ArrayList<>();
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
            items.addAll(snap.filteredReceipts(accountId, Integer.MAX_VALUE));
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
            WidgetStore.ReceiptItem r = items.get(position);
            RemoteViews row = new RemoteViews(context.getPackageName(), R.layout.widget_receipts_item);

            row.setTextViewText(
                    R.id.widget_rcpt_item_merchant, r.merchant.isEmpty() ? Lang.of(context).getString(R.string.widget_receipt) : r.merchant);
            row.setViewVisibility(R.id.widget_rcpt_item_refund, r.refund ? View.VISIBLE : View.GONE);
            row.setTextViewText(
                    R.id.widget_rcpt_item_refund, Lang.of(context).getString(R.string.widget_refund_tag));
            if (r.refund) {
                row.setTextColor(R.id.widget_rcpt_item_refund, accent);
            }

            StringBuilder detail = new StringBuilder();
            if (!r.order.isEmpty()) {
                detail.append(Lang.of(context).getString(R.string.widget_order, r.order));
            }
            String date = WidgetStore.formatMessageDate(context, r.updated > 0 ? r.updated : r.date);
            if (!date.isEmpty()) {
                if (detail.length() > 0) detail.append(" · ");
                detail.append(date);
            }
            if (showAccountLabel && !r.accountLabel.isEmpty()) {
                if (detail.length() > 0) detail.append(" · ");
                detail.append(r.accountLabel);
            }
            row.setTextViewText(R.id.widget_rcpt_item_detail, detail.toString());

            row.setTextViewText(R.id.widget_rcpt_item_amount, r.amount.isEmpty() ? "—" : r.amount);
            row.setTextColor(
                    R.id.widget_rcpt_item_amount,
                    r.refund ? accent : context.getColor(R.color.widget_text));

            String uri = (!r.accountId.isEmpty() && !r.mailId.isEmpty())
                    ? WidgetStore.messageUri(r.accountId, r.mailId)
                    : WidgetStore.receiptsUri(accountId);
            row.setOnClickFillInIntent(R.id.widget_rcpt_item_root, WidgetStore.fillInIntent(context, uri));
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
