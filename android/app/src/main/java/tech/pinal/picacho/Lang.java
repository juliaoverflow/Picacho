package tech.pinal.picacho;

import android.app.LocaleManager;
import android.appwidget.AppWidgetManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.os.Build;
import android.os.LocaleList;
import java.util.Locale;

/**
 * Picacho's language for the text Android shows for it (notifications,
 * widgets, the widget settings and Proton check screens): the language
 * Picacho's own screens show, which Settings may choose apart from the
 * phone's. Rust tells it (src/android.rs, set_language); widgets and
 * notifications read it later, in any process.
 */
final class Lang {
    private static final String PREFS = "picacho_language";
    private static final String KEY = "code";

    private static String cachedCode;
    private static Context cachedContext;

    private Lang() {}

    /**
     * Called from Rust: Picacho shows {@code code} (a BCP 47 tag). With
     * {@code apply}, {@code chosen} was just chosen in Settings (empty: the
     * phone's language), and Android 13 and later learn it too, so its own
     * screens (app info, the widget picker) agree.
     */
    static void set(Context context, String code, String chosen, boolean apply) {
        Context app = context.getApplicationContext();
        String before = prefs(app).getString(KEY, "");
        prefs(app).edit().putString(KEY, code).apply();
        if (apply && Build.VERSION.SDK_INT >= 33) {
            LocaleManager locales = app.getSystemService(LocaleManager.class);
            if (locales != null) {
                locales.setApplicationLocales(chosen.isEmpty()
                        ? LocaleList.getEmptyLocaleList()
                        : LocaleList.forLanguageTags(chosen));
            }
        }
        if (!before.equals(code)) {
            // The widgets' text, and the notification channels' names.
            refreshWidgets(app);
            Notifier.renameChannels(app);
        }
    }

    /** {@code context}, with its resources in Picacho's language. */
    static synchronized Context of(Context context) {
        String code = prefs(context).getString(KEY, "");
        if (code.isEmpty()) {
            return context;
        }
        if (!code.equals(cachedCode) || cachedContext == null) {
            Configuration config = new Configuration(context.getResources().getConfiguration());
            config.setLocales(new LocaleList(Locale.forLanguageTag(code)));
            cachedContext = context.getApplicationContext().createConfigurationContext(config);
            cachedCode = code;
        }
        return cachedContext;
    }

    /** An activity's base context, in Picacho's language (attachBaseContext). */
    static Context wrap(Context base) {
        String code = prefs(base).getString(KEY, "");
        if (code.isEmpty()) {
            return base;
        }
        Configuration config = new Configuration(base.getResources().getConfiguration());
        config.setLocales(new LocaleList(Locale.forLanguageTag(code)));
        return base.createConfigurationContext(config);
    }

    /** Picacho's language, for writing dates. */
    static Locale locale(Context context) {
        String code = prefs(context).getString(KEY, "");
        return code.isEmpty() ? Locale.getDefault() : Locale.forLanguageTag(code);
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private static void refreshWidgets(Context context) {
        AppWidgetManager manager = AppWidgetManager.getInstance(context);
        Class<?>[] providers = {
            InboxWidgetProvider.class,
            UnreadWidgetProvider.class,
            CalendarWidgetProvider.class,
            PackagesWidgetProvider.class,
            ReceiptsWidgetProvider.class,
        };
        for (Class<?> provider : providers) {
            int[] ids = manager.getAppWidgetIds(new ComponentName(context, provider));
            if (ids.length == 0) {
                continue;
            }
            Intent update = new Intent(AppWidgetManager.ACTION_APPWIDGET_UPDATE)
                    .setClass(context, provider)
                    .putExtra(AppWidgetManager.EXTRA_APPWIDGET_IDS, ids);
            context.sendBroadcast(update);
        }
    }
}
