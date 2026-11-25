package tech.pinal.picacho;

import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.Context;
import android.os.Bundle;
import android.view.View;
import android.widget.RemoteViews;
import java.util.List;

/**
 * Compact glanceable Unread Mail widget: shows the unread count, account
 * label, quick Compose button, and (when sized tall/wide enough) the newest
 * unread message.
 */
public final class UnreadWidgetProvider extends AppWidgetProvider {
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
    public void onAppWidgetOptionsChanged(
            Context context, AppWidgetManager manager, int appWidgetId, Bundle newOptions) {
        updateOne(context, manager, appWidgetId, WidgetStore.load(context));
    }

    @Override
    public void onDeleted(Context context, int[] appWidgetIds) {
        WidgetStore.deleteConfig(context, appWidgetIds);
    }

    static void updateAll(Context context, AppWidgetManager manager, int[] appWidgetIds) {
        WidgetStore.Snapshot snap = WidgetStore.load(context);
        for (int id : appWidgetIds) {
            updateOne(context, manager, id, snap);
        }
    }

    static void updateOne(
            Context context, AppWidgetManager manager, int appWidgetId, WidgetStore.Snapshot snap) {
        String accountId = WidgetStore.configuredAccount(context, appWidgetId);
        int accent = WidgetStore.accentColor(context, snap);
        int onAccent = WidgetStore.onAccentColor(context);

        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.widget_unread);
        views.setInt(R.id.widget_unread_logo, "setColorFilter", accent);
        views.setInt(R.id.widget_unread_compose_bg, "setColorFilter", accent);
        views.setInt(R.id.widget_unread_compose_icon, "setColorFilter", onAccent);

        views.setTextViewText(R.id.widget_unread_label, snap.labelFor(context, accountId));

        int unread = snap.unreadFor(accountId);
        views.setTextViewText(
                R.id.widget_unread_count, unread > 999 ? "999+" : String.valueOf(unread));
        views.setTextColor(
                R.id.widget_unread_count,
                unread > 0 ? accent : context.getColor(R.color.widget_text));
        views.setTextViewText(
                R.id.widget_unread_subtitle,
                context.getString(
                        unread == 0 ? R.string.widget_empty_unread : R.string.widget_unread_title));

        Bundle opts = manager.getAppWidgetOptions(appWidgetId);
        int minHeight = opts != null ? opts.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_HEIGHT, 110) : 110;
        List<WidgetStore.Message> latestUnread = snap.filteredMessages(accountId, true, 1);
        if (minHeight >= 105 && !latestUnread.isEmpty()) {
            WidgetStore.Message latest = latestUnread.get(0);
            views.setViewVisibility(R.id.widget_unread_preview, View.VISIBLE);
            views.setTextViewText(
                    R.id.widget_unread_sender,
                    latest.sender.isEmpty() ? "(Unknown sender)" : latest.sender);
            views.setTextViewText(
                    R.id.widget_unread_subject,
                    latest.subject.isEmpty() ? "(No subject)" : latest.subject);
            views.setOnClickPendingIntent(
                    R.id.widget_unread_preview,
                    WidgetStore.openIntent(
                            context,
                            55_000 + appWidgetId,
                            WidgetStore.messageUri(latest.accountId, latest.id)));
        } else {
            views.setViewVisibility(R.id.widget_unread_preview, View.GONE);
        }

        views.setOnClickPendingIntent(
                R.id.widget_unread_root,
                WidgetStore.openIntent(context, 50_000 + appWidgetId, WidgetStore.inboxUri(accountId)));
        views.setOnClickPendingIntent(
                R.id.widget_unread_compose,
                WidgetStore.openIntent(context, 60_000 + appWidgetId, WidgetStore.composeUri(accountId)));

        manager.updateAppWidget(appWidgetId, views);
    }
}
