package tech.pinal.picacho;

import android.content.Context;
import android.content.Intent;
import android.view.View;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;
import java.util.ArrayList;
import java.util.List;

/** Supplies scrollable upcoming calendar event rows to {@link CalendarWidgetProvider}. */
public final class CalendarWidgetService extends RemoteViewsService {
    @Override
    public RemoteViewsFactory onGetViewFactory(Intent intent) {
        return new Factory(getApplicationContext());
    }

    private static final class Factory implements RemoteViewsFactory {
        private final Context context;
        private final List<WidgetStore.Event> events = new ArrayList<>();
        private WidgetStore.Snapshot snap = new WidgetStore.Snapshot();

        Factory(Context context) {
            this.context = context;
        }

        @Override
        public void onCreate() {}

        @Override
        public void onDataSetChanged() {
            snap = WidgetStore.load(context);
            events.clear();
            events.addAll(snap.events);
        }

        @Override
        public void onDestroy() {
            events.clear();
        }

        @Override
        public int getCount() {
            return events.size();
        }

        @Override
        public RemoteViews getViewAt(int position) {
            if (position < 0 || position >= events.size()) {
                return null;
            }
            WidgetStore.Event ev = events.get(position);
            RemoteViews row = new RemoteViews(context.getPackageName(), R.layout.widget_calendar_item);
            int color = WidgetStore.parseColorOrAccent(context, snap, ev.calendarColor);
            row.setInt(R.id.widget_cal_bar, "setColorFilter", color);
            row.setTextViewText(R.id.widget_cal_event_title, ev.title);
            row.setTextViewText(R.id.widget_cal_event_time, WidgetStore.formatEventTime(context, ev));
            if (ev.location.isEmpty()) {
                row.setViewVisibility(R.id.widget_cal_event_location, View.GONE);
            } else {
                row.setViewVisibility(R.id.widget_cal_event_location, View.VISIBLE);
                row.setTextViewText(R.id.widget_cal_event_location, ev.location);
            }
            row.setOnClickFillInIntent(
                    R.id.widget_cal_item_root,
                    WidgetStore.fillInIntent(context, WidgetStore.calendarUri()));
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
            if (position < 0 || position >= events.size()) {
                return position;
            }
            WidgetStore.Event ev = events.get(position);
            return (ev.uid + "@" + ev.start).hashCode();
        }

        @Override
        public boolean hasStableIds() {
            return true;
        }
    }
}
