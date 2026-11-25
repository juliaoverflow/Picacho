package tech.pinal.picacho;

import android.appwidget.AppWidgetManager;
import android.content.Context;
import android.content.Intent;
import android.graphics.Typeface;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.style.StyleSpan;
import android.view.View;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;
import java.util.ArrayList;
import java.util.List;

/** Supplies scrollable inbox message rows to {@link InboxWidgetProvider}. */
public final class InboxWidgetService extends RemoteViewsService {
    @Override
    public RemoteViewsFactory onGetViewFactory(Intent intent) {
        int appWidgetId = intent.getIntExtra(
                AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID);
        return new Factory(getApplicationContext(), appWidgetId);
    }

    private static final class Factory implements RemoteViewsFactory {
        private static final int MAX_ROWS = 25;
        private final Context context;
        private final int appWidgetId;
        private final List<WidgetStore.Message> items = new ArrayList<>();
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
            String accountId = WidgetStore.configuredAccount(context, appWidgetId);
            boolean unreadOnly = WidgetStore.configuredUnreadOnly(context, appWidgetId);
            accent = WidgetStore.accentColor(context, snap);
            items.clear();
            items.addAll(snap.filteredMessages(accountId, unreadOnly, MAX_ROWS));
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
            WidgetStore.Message m = items.get(position);
            RemoteViews row = new RemoteViews(context.getPackageName(), R.layout.widget_inbox_item);

            if (m.unread) {
                row.setViewVisibility(R.id.widget_item_dot, View.VISIBLE);
                row.setInt(R.id.widget_item_dot, "setColorFilter", accent);
            } else {
                row.setViewVisibility(R.id.widget_item_dot, View.INVISIBLE);
            }

            CharSequence senderText = m.sender.isEmpty()
                    ? Lang.of(context).getString(R.string.widget_unknown_sender) : m.sender;
            CharSequence subjectText = m.subject.isEmpty()
                    ? Lang.of(context).getString(R.string.widget_no_subject) : m.subject;
            if (m.unread) {
                senderText = bold(senderText);
                subjectText = bold(subjectText);
                row.setTextColor(R.id.widget_item_sender, context.getColor(R.color.widget_text));
                row.setTextColor(R.id.widget_item_subject, context.getColor(R.color.widget_text));
            } else {
                row.setTextColor(
                        R.id.widget_item_sender, context.getColor(R.color.widget_text_secondary));
                row.setTextColor(
                        R.id.widget_item_subject, context.getColor(R.color.widget_text_secondary));
            }
            row.setTextViewText(R.id.widget_item_sender, senderText);
            row.setTextViewText(R.id.widget_item_subject, subjectText);
            row.setTextViewText(R.id.widget_item_date, WidgetStore.formatMessageDate(context, m.date));

            if (m.snippet.isEmpty()) {
                row.setViewVisibility(R.id.widget_item_snippet, View.GONE);
            } else {
                row.setViewVisibility(R.id.widget_item_snippet, View.VISIBLE);
                row.setTextViewText(R.id.widget_item_snippet, m.snippet);
            }

            row.setViewVisibility(R.id.widget_item_attach, m.hasAttachments ? View.VISIBLE : View.GONE);
            if (m.hasAttachments) {
                row.setInt(
                        R.id.widget_item_attach,
                        "setColorFilter",
                        context.getColor(R.color.widget_text_muted));
            }
            row.setViewVisibility(R.id.widget_item_star, m.starred ? View.VISIBLE : View.GONE);
            if (m.starred) {
                row.setInt(R.id.widget_item_star, "setColorFilter", context.getColor(R.color.widget_star));
            }

            row.setOnClickFillInIntent(
                    R.id.widget_item_root,
                    WidgetStore.fillInIntent(context, WidgetStore.messageUri(m.accountId, m.id)));
            return row;
        }

        private static CharSequence bold(CharSequence text) {
            SpannableString span = new SpannableString(text);
            span.setSpan(new StyleSpan(Typeface.BOLD), 0, span.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            return span;
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
            return items.get(position).id.hashCode();
        }

        @Override
        public boolean hasStableIds() {
            return true;
        }
    }
}
