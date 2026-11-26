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
 * Resizable home-screen Inbox widget: displays the chosen account's (or All
 * Inboxes') unread count, a scrollable list of recent messages, and quick
 * Refresh and Compose actions.
 */
public final class InboxWidgetProvider extends AppWidgetProvider {
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
        boolean unreadOnly = WidgetStore.configuredUnreadOnly(context, appWidgetId);
        int accent = WidgetStore.accentColor(context, snap);
        int onAccent = WidgetStore.onAccentColor(context);

        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.widget_inbox);
        // The layout's own text, in Picacho's language (the launcher would
        // read the layout in the phone's).
        Context text = Lang.of(context);
        views.setContentDescription(R.id.widget_inbox_refresh, text.getString(R.string.widget_refresh));
        views.setContentDescription(R.id.widget_inbox_compose, text.getString(R.string.widget_compose));
        views.setTextViewText(R.id.widget_inbox_empty, text.getString(R.string.widget_empty_inbox));
        views.setInt(R.id.widget_inbox_logo, "setColorFilter", accent);
        views.setInt(R.id.widget_inbox_compose_bg, "setColorFilter", accent);
        views.setInt(R.id.widget_inbox_compose_icon, "setColorFilter", onAccent);
        views.setInt(
                R.id.widget_inbox_refresh,
                "setColorFilter",
                context.getColor(R.color.widget_text_muted));

        String title = snap.labelFor(context, accountId);
        if (unreadOnly) {
            title = title + " · " + Lang.of(context).getString(R.string.widget_unread_title);
        }
        views.setTextViewText(R.id.widget_inbox_title, title);

        int unread = snap.unreadFor(accountId);
        if (unread > 0) {
            views.setViewVisibility(R.id.widget_inbox_badge, View.VISIBLE);
            views.setTextViewText(R.id.widget_inbox_badge, unread > 999 ? "999+" : String.valueOf(unread));
            views.setTextColor(R.id.widget_inbox_badge, accent);
        } else {
            views.setViewVisibility(R.id.widget_inbox_badge, View.GONE);
        }

        int emptyRes = snap.accounts.isEmpty()
                ? R.string.widget_empty_accounts
                : (unreadOnly ? R.string.widget_empty_unread : R.string.widget_empty_inbox);
        views.setTextViewText(R.id.widget_inbox_empty, context.getString(emptyRes));

        // Header opens the configured Inbox; Compose starts a new draft; Refresh reloads.
        views.setOnClickPendingIntent(
                R.id.widget_inbox_header,
                WidgetStore.openIntent(context, 10_000 + appWidgetId, WidgetStore.inboxUri(accountId)));
        views.setOnClickPendingIntent(
                R.id.widget_inbox_empty,
                WidgetStore.openIntent(context, 15_000 + appWidgetId, WidgetStore.inboxUri(accountId)));
        views.setOnClickPendingIntent(
                R.id.widget_inbox_compose,
                WidgetStore.openIntent(context, 20_000 + appWidgetId, WidgetStore.composeUri(accountId)));

        Intent refreshIntent = WidgetStore.refreshIntent(context, InboxWidgetProvider.class);
        PendingIntent refreshPending = PendingIntent.getBroadcast(
                context,
                30_000 + appWidgetId,
                refreshIntent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        views.setOnClickPendingIntent(R.id.widget_inbox_refresh, refreshPending);

        // Scrollable message list backed by InboxWidgetService.
        Intent serviceIntent = new Intent(context, InboxWidgetService.class)
                .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId);
        serviceIntent.setData(Uri.parse(serviceIntent.toUri(Intent.URI_INTENT_SCHEME)));
        views.setRemoteAdapter(R.id.widget_inbox_list, serviceIntent);
        views.setEmptyView(R.id.widget_inbox_list, R.id.widget_inbox_empty);
        views.setPendingIntentTemplate(
                R.id.widget_inbox_list, WidgetStore.templateIntent(context, 40_000 + appWidgetId));

        manager.updateAppWidget(appWidgetId, views);
    }
}
