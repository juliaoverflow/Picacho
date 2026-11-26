package tech.pinal.picacho;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.content.BroadcastReceiver;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Color;
import android.net.Uri;
import android.os.Build;
import android.util.Log;
import java.io.File;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.text.DateFormat;
import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.Date;
import java.util.List;
import java.util.Locale;
import java.util.TimeZone;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Shared state and helpers for Picacho's home-screen widgets: reads
 * {@code widget-snapshot.json} written by Rust ({@code manage::widgets}),
 * stores per-widget account/filter preferences, formats dates and accent
 * colors, and builds authenticated deep-link intents back into
 * {@link PicachoActivity}.
 */
public final class WidgetStore {
    private static final String TAG = "picacho";
    private static final String SNAPSHOT_NAME = "widget-snapshot.json";
    /** Minimum snapshot schema version that includes per-account packages & receipts. */
    static final int MIN_VERSION = 2;
    private static final String PREFS = "widgets";
    /** Deep-link URI ({@code picacho://widget/...}) carried by a widget tap. */
    static final String EXTRA_WIDGET = "tech.pinal.picacho.WIDGET";
    /**
     * The widgets' own key, carried by their taps. It is not
     * {@link Notifier#key}: a collection widget's row intents are handed to
     * the home-screen app, which may be anyone's, so this key only lets a
     * {@code picacho://widget/...} view be opened (never a notification's
     * target, whose actions archive or delete mail).
     */
    static final String EXTRA_KEY = "tech.pinal.picacho.WIDGET_KEY";
    private static final String KEY = "widget_key";
    /** Broadcast action for a widget's Refresh button. */
    static final String ACTION_REFRESH = "tech.pinal.picacho.action.WIDGET_REFRESH";

    private WidgetStore() {}

    static final class Snapshot {
        int version;
        int accentLight = 0x246846;
        int accentDark = 0x86cfaa;
        int totalUnread;
        boolean calendarConnected;
        final List<Account> accounts = new ArrayList<>();
        final List<Message> messages = new ArrayList<>();
        final List<Event> events = new ArrayList<>();
        final List<TrackedPackage> packages = new ArrayList<>();
        final List<ReceiptItem> receipts = new ArrayList<>();

        int unreadFor(String accountId) {
            if (accountId == null || accountId.isEmpty()) {
                return totalUnread;
            }
            for (Account a : accounts) {
                if (accountId.equals(a.id)) {
                    return a.unread;
                }
            }
            return 0;
        }

        String labelFor(Context context, String accountId) {
            if (accountId == null || accountId.isEmpty()) {
                if (accounts.size() == 1) {
                    return accounts.get(0).name;
                }
                return Lang.of(context).getString(R.string.widget_all_inboxes);
            }
            for (Account a : accounts) {
                if (accountId.equals(a.id)) {
                    return a.name;
                }
            }
            return Lang.of(context).getString(R.string.widget_inbox_title);
        }

        String accountLabelFor(Context context, String accountId) {
            if (accountId == null || accountId.isEmpty()) {
                if (accounts.size() == 1) {
                    return accounts.get(0).name;
                }
                return Lang.of(context).getString(R.string.widget_all_accounts);
            }
            for (Account a : accounts) {
                if (accountId.equals(a.id)) {
                    return a.name;
                }
            }
            return Lang.of(context).getString(R.string.widget_all_accounts);
        }

        List<Message> filteredMessages(String accountId, boolean unreadOnly, int limit) {
            List<Message> out = new ArrayList<>();
            boolean hasFilter = accountId != null && !accountId.isEmpty();
            for (Message m : messages) {
                if (hasFilter && !accountId.equals(m.accountId)) {
                    continue;
                }
                if (unreadOnly && !m.unread) {
                    continue;
                }
                out.add(m);
                if (out.size() >= limit) {
                    break;
                }
            }
            return out;
        }

        List<TrackedPackage> filteredPackages(String accountId, int limit) {
            List<TrackedPackage> out = new ArrayList<>();
            boolean hasFilter = accountId != null && !accountId.isEmpty();
            for (TrackedPackage p : packages) {
                if (hasFilter && !accountId.equals(p.accountId)) {
                    continue;
                }
                out.add(p);
                if (out.size() >= limit) {
                    break;
                }
            }
            return out;
        }

        /**
         * How many packages the Summary lists for one account (or All
         * Accounts, for none): the "View all" count. Never fewer than the
         * snapshot holds.
         */
        int packageTotal(String accountId) {
            boolean hasFilter = accountId != null && !accountId.isEmpty();
            int total = 0;
            for (Account a : accounts) {
                if (!hasFilter || accountId.equals(a.id)) {
                    total += a.packages;
                }
            }
            return Math.max(total, filteredPackages(accountId, Integer.MAX_VALUE).size());
        }

        /** How many receipts the Summary lists for one account, or all. */
        int receiptTotal(String accountId) {
            boolean hasFilter = accountId != null && !accountId.isEmpty();
            int total = 0;
            for (Account a : accounts) {
                if (!hasFilter || accountId.equals(a.id)) {
                    total += a.receipts;
                }
            }
            return Math.max(total, filteredReceipts(accountId, Integer.MAX_VALUE).size());
        }

        List<ReceiptItem> filteredReceipts(String accountId, int limit) {
            List<ReceiptItem> out = new ArrayList<>();
            boolean hasFilter = accountId != null && !accountId.isEmpty();
            for (ReceiptItem r : receipts) {
                if (hasFilter && !accountId.equals(r.accountId)) {
                    continue;
                }
                out.add(r);
                if (out.size() >= limit) {
                    break;
                }
            }
            return out;
        }
    }

    static final class Account {
        final String id;
        final String name;
        final String email;
        final int unread;
        /** Packages and receipts the Summary lists for this account. */
        final int packages;
        final int receipts;

        Account(String id, String name, String email, int unread, int packages, int receipts) {
            this.id = id;
            this.name = name;
            this.email = email;
            this.unread = unread;
            this.packages = packages;
            this.receipts = receipts;
        }
    }

    static final class Message {
        final String id;
        final String accountId;
        final String sender;
        final String subject;
        final String snippet;
        final long date;
        final boolean unread;
        final boolean starred;
        final boolean hasAttachments;

        Message(
                String id,
                String accountId,
                String sender,
                String subject,
                String snippet,
                long date,
                boolean unread,
                boolean starred,
                boolean hasAttachments) {
            this.id = id;
            this.accountId = accountId;
            this.sender = sender;
            this.subject = subject;
            this.snippet = snippet;
            this.date = date;
            this.unread = unread;
            this.starred = starred;
            this.hasAttachments = hasAttachments;
        }
    }

    static final class Event {
        final String uid;
        final String title;
        final String location;
        final long start;
        final long end;
        final boolean allDay;
        final String calendarName;
        final String calendarColor;

        Event(
                String uid,
                String title,
                String location,
                long start,
                long end,
                boolean allDay,
                String calendarName,
                String calendarColor) {
            this.uid = uid;
            this.title = title;
            this.location = location;
            this.start = start;
            this.end = end;
            this.allDay = allDay;
            this.calendarName = calendarName;
            this.calendarColor = calendarColor;
        }
    }

    static final class TrackedPackage {
        final String key;
        final String title;
        final String number;
        final String carrier;
        final String status;
        final int stage;
        final boolean delivered;
        final String expected;
        final long updated;
        final String accountId;
        final String accountLabel;
        final String mailId;

        TrackedPackage(
                String key,
                String title,
                String number,
                String carrier,
                String status,
                int stage,
                boolean delivered,
                String expected,
                long updated,
                String accountId,
                String accountLabel,
                String mailId) {
            this.key = key;
            this.title = title;
            this.number = number;
            this.carrier = carrier;
            this.status = status;
            this.stage = stage;
            this.delivered = delivered;
            this.expected = expected;
            this.updated = updated;
            this.accountId = accountId;
            this.accountLabel = accountLabel;
            this.mailId = mailId;
        }
    }

    static final class ReceiptItem {
        final String key;
        final String merchant;
        final String amount;
        final boolean refund;
        final String order;
        final long date;
        final long updated;
        final String accountId;
        final String accountLabel;
        final String mailId;

        ReceiptItem(
                String key,
                String merchant,
                String amount,
                boolean refund,
                String order,
                long date,
                long updated,
                String accountId,
                String accountLabel,
                String mailId) {
            this.key = key;
            this.merchant = merchant;
            this.amount = amount;
            this.refund = refund;
            this.order = order;
            this.date = date;
            this.updated = updated;
            this.accountId = accountId;
            this.accountLabel = accountLabel;
            this.mailId = mailId;
        }
    }

    static File snapshotFile(Context context) {
        return new File(new File(context.getFilesDir(), "picacho"), SNAPSHOT_NAME);
    }

    static boolean needsRefresh(Context context, Snapshot snap) {
        return !snapshotFile(context).isFile() || snap.version < MIN_VERSION;
    }

    static Snapshot load(Context context) {
        Snapshot snap = new Snapshot();
        File file = snapshotFile(context);
        if (!file.isFile()) {
            return snap;
        }
        try (FileInputStream in = new FileInputStream(file)) {
            byte[] bytes = new byte[(int) Math.min(file.length(), 2 * 1024 * 1024)];
            int offset = 0;
            while (offset < bytes.length) {
                int n = in.read(bytes, offset, bytes.length - offset);
                if (n < 0) {
                    break;
                }
                offset += n;
            }
            JSONObject root = new JSONObject(new String(bytes, 0, offset, StandardCharsets.UTF_8));
            snap.version = root.optInt("version", 1);
            snap.accentLight = root.optInt("accent_light", snap.accentLight);
            snap.accentDark = root.optInt("accent_dark", snap.accentDark);
            snap.totalUnread = root.optInt("total_unread", 0);
            snap.calendarConnected = root.optBoolean("calendar_connected", false);

            JSONArray accs = root.optJSONArray("accounts");
            if (accs != null) {
                for (int i = 0; i < accs.length(); i++) {
                    JSONObject o = accs.optJSONObject(i);
                    if (o != null) {
                        snap.accounts.add(new Account(
                                o.optString("id", ""),
                                o.optString("name", ""),
                                o.optString("email", ""),
                                o.optInt("unread", 0),
                                o.optInt("packages", 0),
                                o.optInt("receipts", 0)));
                    }
                }
            }
            JSONArray msgs = root.optJSONArray("messages");
            if (msgs != null) {
                for (int i = 0; i < msgs.length(); i++) {
                    JSONObject o = msgs.optJSONObject(i);
                    if (o != null) {
                        snap.messages.add(new Message(
                                o.optString("id", ""),
                                o.optString("account_id", ""),
                                o.optString("sender", ""),
                                o.optString("subject", ""),
                                o.optString("snippet", ""),
                                o.optLong("date", 0L),
                                o.optBoolean("unread", false),
                                o.optBoolean("starred", false),
                                o.optBoolean("has_attachments", false)));
                    }
                }
            }
            JSONArray evs = root.optJSONArray("events");
            if (evs != null) {
                for (int i = 0; i < evs.length(); i++) {
                    JSONObject o = evs.optJSONObject(i);
                    if (o != null) {
                        snap.events.add(new Event(
                                o.optString("uid", ""),
                                o.optString("title", ""),
                                o.optString("location", ""),
                                o.optLong("start", 0L),
                                o.optLong("end", 0L),
                                o.optBoolean("all_day", false),
                                o.optString("calendar_name", ""),
                                o.optString("calendar_color", "")));
                    }
                }
            }
            JSONArray pkgs = root.optJSONArray("packages");
            if (pkgs != null) {
                for (int i = 0; i < pkgs.length(); i++) {
                    JSONObject o = pkgs.optJSONObject(i);
                    if (o != null) {
                        snap.packages.add(new TrackedPackage(
                                o.optString("key", ""),
                                o.optString("title", ""),
                                o.optString("number", ""),
                                o.optString("carrier", ""),
                                o.optString("status", ""),
                                o.optInt("stage", 0),
                                o.optBoolean("delivered", false),
                                o.optString("expected", ""),
                                o.optLong("updated", 0L),
                                o.optString("account_id", ""),
                                o.optString("account_label", ""),
                                o.optString("mail_id", "")));
                    }
                }
            }
            JSONArray rcpts = root.optJSONArray("receipts");
            if (rcpts != null) {
                for (int i = 0; i < rcpts.length(); i++) {
                    JSONObject o = rcpts.optJSONObject(i);
                    if (o != null) {
                        long date = o.optLong("date", 0L);
                        long updated = o.optLong("updated", date);
                        snap.receipts.add(new ReceiptItem(
                                o.optString("key", ""),
                                o.optString("merchant", ""),
                                o.optString("amount", ""),
                                o.optBoolean("refund", false),
                                o.optString("order", ""),
                                date,
                                updated > 0 ? updated : date,
                                o.optString("account_id", ""),
                                o.optString("account_label", ""),
                                o.optString("mail_id", "")));
                    }
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "could not read widget snapshot", e);
        }
        return snap;
    }

    /** Called from Rust (via JNI) and from Android receivers when data changes. */
    public static void refreshAll(Context context) {
        Context app = context.getApplicationContext();
        AppWidgetManager manager = AppWidgetManager.getInstance(app);
        if (manager == null) {
            return;
        }
        // The snapshot read once for all of them (each read and parsed it).
        Snapshot snap = load(app);
        int[] inboxIds = manager.getAppWidgetIds(new ComponentName(app, InboxWidgetProvider.class));
        if (inboxIds.length > 0) {
            manager.notifyAppWidgetViewDataChanged(inboxIds, R.id.widget_inbox_list);
            InboxWidgetProvider.updateAll(app, manager, inboxIds, snap);
        }
        int[] unreadIds = manager.getAppWidgetIds(new ComponentName(app, UnreadWidgetProvider.class));
        if (unreadIds.length > 0) {
            UnreadWidgetProvider.updateAll(app, manager, unreadIds, snap);
        }
        int[] calIds = manager.getAppWidgetIds(new ComponentName(app, CalendarWidgetProvider.class));
        if (calIds.length > 0) {
            manager.notifyAppWidgetViewDataChanged(calIds, R.id.widget_cal_list);
            CalendarWidgetProvider.updateAll(app, manager, calIds, snap);
        }
        int[] pkgIds = manager.getAppWidgetIds(new ComponentName(app, PackagesWidgetProvider.class));
        if (pkgIds.length > 0) {
            manager.notifyAppWidgetViewDataChanged(pkgIds, R.id.widget_pkg_list);
            PackagesWidgetProvider.updateAll(app, manager, pkgIds, snap);
        }
        int[] rcptIds = manager.getAppWidgetIds(new ComponentName(app, ReceiptsWidgetProvider.class));
        if (rcptIds.length > 0) {
            manager.notifyAppWidgetViewDataChanged(rcptIds, R.id.widget_rcpt_list);
            ReceiptsWidgetProvider.updateAll(app, manager, rcptIds, snap);
        }
    }

    /**
     * Rebuilds {@code widget-snapshot.json} from SQLite on a background thread
     * (used on Refresh button taps or when no snapshot exists yet).
     */
    static void refreshFromStoreAsync(Context context, BroadcastReceiver receiver) {
        Context app = context.getApplicationContext();
        // One refresh at a time, and at most one more waiting: taps (or
        // broadcasts) in a burst run one after another, not a thread each.
        if (!REFRESH_WAITING.compareAndSet(false, true)) {
            return;
        }
        BroadcastReceiver.PendingResult pending = receiver != null ? receiver.goAsync() : null;
        REFRESHER.execute(() -> {
            REFRESH_WAITING.set(false);
            try {
                Native.attach(app);
                Native.refreshWidgets(new File(app.getFilesDir(), "picacho").getPath());
            } catch (Throwable e) {
                Log.w(TAG, "could not refresh widgets from store", e);
                refreshAll(app);
            } finally {
                if (pending != null) {
                    pending.finish();
                }
            }
        });
    }

    /** The thread widget refreshes run on, one after another. */
    private static final ExecutorService REFRESHER = Executors.newSingleThreadExecutor(runnable -> {
        Thread thread = new Thread(runnable, "picacho-widget-refresh");
        thread.setDaemon(true);
        return thread;
    });
    /** A refresh is queued and hasn't started: another needn't be. */
    private static final AtomicBoolean REFRESH_WAITING = new AtomicBoolean(false);

    /**
     * A widget's Refresh button: an explicit broadcast to {@code provider}
     * carrying the widgets' key, which {@link #isRefresh} asks for. The
     * providers are exported (the launcher must reach them), so anyone
     * could send them a refresh otherwise.
     */
    static Intent refreshIntent(Context context, Class<?> provider) {
        return new Intent(context, provider)
                .setAction(ACTION_REFRESH)
                .putExtra(EXTRA_KEY, key(context));
    }

    /** Whether {@code intent} is a Refresh from Picacho's own widget. */
    static boolean isRefresh(Context context, Intent intent) {
        if (intent == null || !ACTION_REFRESH.equals(intent.getAction())) {
            return false;
        }
        String given;
        try {
            given = intent.getStringExtra(EXTRA_KEY);
        } catch (RuntimeException e) {
            // A malformed extra from elsewhere.
            return false;
        }
        return given != null
                && MessageDigest.isEqual(
                        given.getBytes(StandardCharsets.UTF_8),
                        key(context).getBytes(StandardCharsets.UTF_8));
    }

    static boolean isDark(Context context) {
        return (context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
                == Configuration.UI_MODE_NIGHT_YES;
    }

    static int accentColor(Context context, Snapshot snap) {
        int rgb = isDark(context) ? snap.accentDark : snap.accentLight;
        return 0xFF000000 | (rgb & 0x00FFFFFF);
    }

    static int onAccentColor(Context context) {
        return context.getColor(R.color.widget_on_accent_default);
    }

    static int parseColorOrAccent(Context context, Snapshot snap, String hex) {
        if (hex != null && hex.startsWith("#") && (hex.length() == 7 || hex.length() == 9)) {
            try {
                return Color.parseColor(hex);
            } catch (IllegalArgumentException ignored) {
                // Fall back to Picacho's theme accent.
            }
        }
        return accentColor(context, snap);
    }

    /** The widgets' key (see {@link #EXTRA_KEY}), made on first use. */
    static synchronized String key(Context context) {
        SharedPreferences preferences = prefs(context);
        String key = preferences.getString(KEY, null);
        if (key == null || key.isEmpty()) {
            byte[] random = new byte[32];
            new SecureRandom().nextBytes(random);
            StringBuilder hex = new StringBuilder();
            for (byte b : random) {
                hex.append(String.format("%02x", b));
            }
            key = hex.toString();
            // Written at once: the restart process (RestartActivity) reads it.
            preferences.edit().putString(KEY, key).commit();
        }
        return key;
    }

    // --- Per-widget configuration ---

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static String configuredAccount(Context context, int appWidgetId) {
        return prefs(context).getString("account_" + appWidgetId, "");
    }

    static boolean configuredUnreadOnly(Context context, int appWidgetId) {
        return prefs(context).getBoolean("unread_" + appWidgetId, false);
    }

    static void saveConfig(Context context, int appWidgetId, String accountId, boolean unreadOnly) {
        prefs(context)
                .edit()
                .putString("account_" + appWidgetId, accountId == null ? "" : accountId)
                .putBoolean("unread_" + appWidgetId, unreadOnly)
                .apply();
    }

    static void deleteConfig(Context context, int[] appWidgetIds) {
        SharedPreferences.Editor editor = prefs(context).edit();
        for (int id : appWidgetIds) {
            editor.remove("account_" + id).remove("unread_" + id);
        }
        editor.apply();
    }

    // --- Deep links ---

    static String inboxUri(String accountId) {
        if (accountId == null || accountId.isEmpty()) {
            return "picacho://widget/inbox";
        }
        return "picacho://widget/inbox?account=" + Uri.encode(accountId);
    }

    static String composeUri(String accountId) {
        if (accountId == null || accountId.isEmpty()) {
            return "picacho://widget/compose";
        }
        return "picacho://widget/compose?account=" + Uri.encode(accountId);
    }

    static String messageUri(String accountId, String messageId) {
        return "picacho://widget/message?account="
                + Uri.encode(accountId)
                + "&id="
                + Uri.encode(messageId);
    }

    static String calendarUri() {
        return "picacho://widget/calendar";
    }

    static String packagesUri(String accountId) {
        if (accountId == null || accountId.isEmpty()) {
            return "picacho://widget/packages";
        }
        return "picacho://widget/packages?account=" + Uri.encode(accountId);
    }

    static String receiptsUri(String accountId) {
        if (accountId == null || accountId.isEmpty()) {
            return "picacho://widget/receipts";
        }
        return "picacho://widget/receipts?account=" + Uri.encode(accountId);
    }

    /** Direct PendingIntent that opens Picacho with an authenticated widget URI. */
    static PendingIntent openIntent(Context context, int requestCode, String widgetUri) {
        Intent open = new Intent(Intent.ACTION_MAIN)
                .setClassName(context, "android.app.NativeActivity")
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_WIDGET, widgetUri)
                .putExtra(EXTRA_KEY, key(context));
        return PendingIntent.getActivity(
                context,
                requestCode,
                open,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    /**
     * Explicit mutable PendingIntent template for collection widget rows
     * ({@code ListView}); each row supplies its URI and auth key via
     * {@link #fillInIntent}.
     */
    static PendingIntent templateIntent(Context context, int requestCode) {
        Intent open = new Intent(Intent.ACTION_MAIN)
                .setClassName(context, "android.app.NativeActivity")
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (Build.VERSION.SDK_INT >= 31) {
            flags |= PendingIntent.FLAG_MUTABLE;
        }
        return PendingIntent.getActivity(context, requestCode, open, flags);
    }

    static Intent fillInIntent(Context context, String widgetUri) {
        return new Intent()
                .putExtra(EXTRA_WIDGET, widgetUri)
                .putExtra(EXTRA_KEY, key(context));
    }

    private static final String WIDGET_URI_PREFIX = "picacho://widget/";

    /** Extracts a verified widget URI from a launch Intent, or null. */
    static String widgetUriOf(Context context, Intent intent) {
        if (intent == null
                || (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0) {
            return null;
        }
        String uri = intent.getStringExtra(EXTRA_WIDGET);
        // Only a widget view: Native.launch also takes other requests (such
        // as a notification's target), which the widget key mustn't reach.
        if (uri == null || !uri.startsWith(WIDGET_URI_PREFIX) || uri.length() > 4096) {
            return null;
        }
        String given = intent.getStringExtra(EXTRA_KEY);
        if (given == null
                || !MessageDigest.isEqual(
                        given.getBytes(StandardCharsets.UTF_8),
                        key(context).getBytes(StandardCharsets.UTF_8))) {
            return null;
        }
        return uri;
    }

    // --- Date and time formatting ---

    static String formatMessageDate(Context context, long epochSeconds) {
        if (epochSeconds <= 0) {
            return "";
        }
        Date date = new Date(epochSeconds * 1000L);
        Calendar now = Calendar.getInstance();
        Calendar then = Calendar.getInstance();
        then.setTime(date);
        if (sameDay(now, then)) {
            return android.text.format.DateFormat.getTimeFormat(Lang.of(context)).format(date);
        }
        long diffDays = (now.getTimeInMillis() - then.getTimeInMillis()) / 86_400_000L;
        if (diffDays >= 0 && diffDays < 6) {
            return localDate(context, "EEE").format(date);
        }
        if (now.get(Calendar.YEAR) == then.get(Calendar.YEAR)) {
            return localDate(context, "MMMd").format(date);
        }
        return localDate(context, "yMMMd").format(date);
    }

    static String formatTodayHeader(Context context) {
        return localDate(context, "EEEMMMd").format(new Date());
    }

    /** A date format with these fields, in the order and words of the phone's language. */
    private static SimpleDateFormat localDate(Context context, String skeleton) {
        Locale locale = Lang.locale(context);
        return new SimpleDateFormat(
                android.text.format.DateFormat.getBestDateTimePattern(locale, skeleton), locale);
    }

    static String formatEventTime(Context context, Event ev) {
        Calendar now = Calendar.getInstance();
        Calendar tomorrow = Calendar.getInstance();
        tomorrow.add(Calendar.DAY_OF_YEAR, 1);

        if (ev.allDay) {
            Calendar startUtc = Calendar.getInstance(TimeZone.getTimeZone("UTC"));
            startUtc.setTimeInMillis(ev.start * 1000L);
            String day = dayLabel(context, now, tomorrow, startUtc);
            return day + " · " + Lang.of(context).getString(R.string.widget_all_day);
        }
        Date start = new Date(ev.start * 1000L);
        Calendar startCal = Calendar.getInstance();
        startCal.setTime(start);
        String day = dayLabel(context, now, tomorrow, startCal);
        DateFormat tf = android.text.format.DateFormat.getTimeFormat(Lang.of(context));
        if (ev.end > ev.start) {
            Date end = new Date(ev.end * 1000L);
            return day + " · " + tf.format(start) + " – " + tf.format(end);
        }
        return day + " · " + tf.format(start);
    }

    private static String dayLabel(
            Context context, Calendar today, Calendar tomorrow, Calendar target) {
        if (sameDay(today, target)) {
            return Lang.of(context).getString(R.string.widget_today);
        }
        if (sameDay(tomorrow, target)) {
            return Lang.of(context).getString(R.string.widget_tomorrow);
        }
        SimpleDateFormat fmt = localDate(context, "EEEMMMd");
        fmt.setTimeZone(target.getTimeZone());
        return fmt.format(target.getTime());
    }

    private static boolean sameDay(Calendar a, Calendar b) {
        return a.get(Calendar.YEAR) == b.get(Calendar.YEAR)
                && a.get(Calendar.DAY_OF_YEAR) == b.get(Calendar.DAY_OF_YEAR);
    }
}
