package tech.pinal.picacho;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.view.View;
import android.widget.RemoteViews;

/**
 * Resizable home-screen Receipts widget: displays your recent receipts and
 * refunds.
 */
public final class ReceiptsWidgetProvider extends AppWidgetProvider {
    @Override
    public void onUpdate(Context context, AppWidgetManager manager, int[] appWidgetIds) {
        WidgetStore.Snapshot snap = WidgetStore.load(context);
        for (int id : appWidgetIds) {
            updateOne(context, manager, id, snap);
        }
        if (WidgetStore.needsRefresh(context, snap)) {
            WidgetStore.refreshFromStoreAsync(context, this);
        }
    }

    @Override
    public void onReceive(Context context, Intent intent) {
        super.onReceive(context, intent);
        if (WidgetStore.isRefresh(context, intent)) {
            WidgetStore.refreshFromStoreAsync(context, this);
        }
    }

    @Override
    public void onDeleted(Context context, int[] appWidgetIds) {
        WidgetStore.deleteConfig(context, appWidgetIds);
    }

    static void updateAll(Context context, AppWidgetManager manager, int[] appWidgetIds) {
        updateAll(context, manager, appWidgetIds, WidgetStore.load(context));
    }

    static void updateAll(
            Context context, AppWidgetManager manager, int[] appWidgetIds, WidgetStore.Snapshot snap) {
        for (int id : appWidgetIds) {
            updateOne(context, manager, id, snap);
        }
    }

    static void updateOne(
            Context context, AppWidgetManager manager, int appWidgetId, WidgetStore.Snapshot snap) {
        String accountId = WidgetStore.configuredAccount(context, appWidgetId);
        int accent = WidgetStore.accentColor(context, snap);

        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.widget_receipts);
        // The layout's own text, in Picacho's language (the launcher would
        // read the layout in the phone's).
        Context text = Lang.of(context);
        views.setContentDescription(R.id.widget_rcpt_refresh, text.getString(R.string.widget_refresh));
        views.setTextViewText(R.id.widget_rcpt_empty, text.getString(R.string.widget_empty_receipts));
        views.setInt(R.id.widget_rcpt_icon, "setColorFilter", accent);
        views.setInt(
                R.id.widget_rcpt_refresh,
                "setColorFilter",
                context.getColor(R.color.widget_text_muted));

        String baseTitle = Lang.of(context).getString(R.string.widget_receipts_label);
        String title = (accountId != null && !accountId.isEmpty())
                ? baseTitle + " · " + snap.accountLabelFor(context, accountId)
                : baseTitle;
        views.setTextViewText(R.id.widget_rcpt_title, title);

        PendingIntent openReceipts = WidgetStore.openIntent(
                context, 110_000 + appWidgetId, WidgetStore.receiptsUri(accountId));
        views.setOnClickPendingIntent(R.id.widget_rcpt_header, openReceipts);
        views.setOnClickPendingIntent(R.id.widget_rcpt_empty, openReceipts);

        // "View all N" in the header opens them all in the Summary.
        int total = snap.receiptTotal(accountId);
        if (total > 0) {
            views.setViewVisibility(R.id.widget_rcpt_view_all, View.VISIBLE);
            views.setTextViewText(
                    R.id.widget_rcpt_view_all,
                    Lang.of(context).getResources().getQuantityString(R.plurals.widget_view_all, total, total));
            views.setTextColor(R.id.widget_rcpt_view_all, accent);
            views.setOnClickPendingIntent(R.id.widget_rcpt_view_all, openReceipts);
        } else {
            views.setViewVisibility(R.id.widget_rcpt_view_all, View.GONE);
        }

        Intent refreshIntent = WidgetStore.refreshIntent(context, ReceiptsWidgetProvider.class);
        PendingIntent refreshPending = PendingIntent.getBroadcast(
                context,
                115_000 + appWidgetId,
                refreshIntent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        views.setOnClickPendingIntent(R.id.widget_rcpt_refresh, refreshPending);

        Intent serviceIntent = new Intent(context, ReceiptsWidgetService.class)
                .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId);
        serviceIntent.setData(Uri.parse(serviceIntent.toUri(Intent.URI_INTENT_SCHEME)));
        views.setRemoteAdapter(R.id.widget_rcpt_list, serviceIntent);
        views.setEmptyView(R.id.widget_rcpt_list, R.id.widget_rcpt_empty);
        views.setPendingIntentTemplate(
                R.id.widget_rcpt_list, WidgetStore.templateIntent(context, 120_000 + appWidgetId));

        manager.updateAppWidget(appWidgetId, views);
    }
}
