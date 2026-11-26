package tech.pinal.picacho;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.widget.RemoteViews;

/**
 * Resizable home-screen Calendar widget: displays upcoming events from
 * Picacho's connected CalDAV/JMAP calendars with their calendar color bars.
 */
public final class CalendarWidgetProvider extends AppWidgetProvider {
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
        int accent = WidgetStore.accentColor(context, snap);

        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.widget_calendar);
        // The layout's own text, in Picacho's language (the launcher would
        // read the layout in the phone's).
        Context text = Lang.of(context);
        views.setTextViewText(R.id.widget_cal_title, text.getString(R.string.widget_calendar_label));
        views.setContentDescription(R.id.widget_cal_refresh, text.getString(R.string.widget_refresh));
        views.setTextViewText(R.id.widget_cal_empty, text.getString(R.string.widget_empty_calendar));
        views.setInt(R.id.widget_cal_icon, "setColorFilter", accent);
        views.setInt(
                R.id.widget_cal_refresh,
                "setColorFilter",
                context.getColor(R.color.widget_text_muted));
        views.setTextViewText(R.id.widget_cal_date, WidgetStore.formatTodayHeader(context));
        views.setTextViewText(
                R.id.widget_cal_empty,
                context.getString(
                        snap.calendarConnected
                                ? R.string.widget_empty_calendar
                                : R.string.widget_no_calendar));

        PendingIntent openCalendar =
                WidgetStore.openIntent(context, 70_000 + appWidgetId, WidgetStore.calendarUri());
        views.setOnClickPendingIntent(R.id.widget_cal_header, openCalendar);
        views.setOnClickPendingIntent(R.id.widget_cal_empty, openCalendar);

        Intent refreshIntent = WidgetStore.refreshIntent(context, CalendarWidgetProvider.class);
        PendingIntent refreshPending = PendingIntent.getBroadcast(
                context,
                75_000 + appWidgetId,
                refreshIntent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        views.setOnClickPendingIntent(R.id.widget_cal_refresh, refreshPending);

        Intent serviceIntent = new Intent(context, CalendarWidgetService.class)
                .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId);
        serviceIntent.setData(Uri.parse(serviceIntent.toUri(Intent.URI_INTENT_SCHEME)));
        views.setRemoteAdapter(R.id.widget_cal_list, serviceIntent);
        views.setEmptyView(R.id.widget_cal_list, R.id.widget_cal_empty);
        views.setPendingIntentTemplate(
                R.id.widget_cal_list, WidgetStore.templateIntent(context, 80_000 + appWidgetId));

        manager.updateAppWidget(appWidgetId, views);
    }
}
