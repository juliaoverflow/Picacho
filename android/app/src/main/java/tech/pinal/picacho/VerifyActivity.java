package tech.pinal.picacho;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.view.MenuItem;
import android.view.WindowInsets;
import android.webkit.CookieManager;
import android.webkit.WebMessage;
import android.webkit.WebMessagePort;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebStorage;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import java.security.SecureRandom;

/**
 * Proton's human verification (its CAPTCHA, or a code by email or text) in
 * a WebView, for Rust (android::verify; display/verify.rs on desktops).
 *
 * <p>Proton's page reports its result with postMessage and, in Proton's own
 * apps, through {@code window.AndroidInterface}. A script in the page's top
 * frame relays both to Rust over a message channel, whose port only that
 * frame receives (with a secret it was given), and only messages from
 * Proton's own origins: the CAPTCHA frames Proton embeds (hCaptcha, Google)
 * can post too. Rust keeps only the result. Nothing the page stores stays.
 */
public final class VerifyActivity extends Activity {
    private static final String EXTRA_ID = "id";
    private static final String EXTRA_URL = "url";
    private static final String PAGE = "https://verify.proton.me/";

    /** The page showing, for Rust to close. UI thread writes. */
    private static volatile VerifyActivity current;

    private int id;
    private boolean answered;
    private WebView web;
    /** Rust's end of the page's channel; kept so it isn't collected. */
    private WebMessagePort port;

    /** Called from Rust: shows {@code url} for request {@code id}. */
    public static boolean start(Context context, int id, String url) {
        if (!url.startsWith(PAGE)) {
            return false;
        }
        Activity from = PicachoActivity.current();
        Intent intent = new Intent(from != null ? from : context, VerifyActivity.class)
                .putExtra(EXTRA_ID, id)
                .putExtra(EXTRA_URL, url);
        try {
            if (from != null) {
                from.startActivity(intent);
            } else {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            }
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    /** Called from Rust once it has what it waited for, or gave up. */
    public static void close(int id) {
        VerifyActivity activity = current;
        if (activity != null && activity.id == id) {
            activity.runOnUiThread(() -> {
                activity.answered = true;
                activity.finish();
            });
        }
    }

    /** The page may only go to Proton and the CAPTCHA services it embeds. */
    static boolean allowed(Uri uri) {
        if ("about:blank".equals(uri.toString())) {
            return true;
        }
        if (!"https".equals(uri.getScheme()) || uri.getHost() == null) {
            return false;
        }
        String host = uri.getHost();
        for (String domain : new String[] {
                "proton.me", "protonmail.com", "protonmail.ch", "hcaptcha.com", "recaptcha.net"}) {
            if (under(host, domain)) {
                return true;
            }
        }
        String path = uri.getPath() == null ? "" : uri.getPath();
        return (host.equals("www.google.com") && path.startsWith("/recaptcha/"))
                || host.equals("www.gstatic.com");
    }

    private static boolean under(String host, String domain) {
        return host.equals(domain) || host.endsWith("." + domain);
    }

    private static boolean proton(Uri uri) {
        return "https".equals(uri.getScheme())
                && uri.getHost() != null
                && (under(uri.getHost(), "proton.me") || under(uri.getHost(), "protonmail.com"));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        id = getIntent().getIntExtra(EXTRA_ID, 0);
        String url = getIntent().getStringExtra(EXTRA_URL);
        if (savedInstanceState != null || url == null || !url.startsWith(PAGE)) {
            // Recreated in a new process: Rust's request is gone with the old one.
            answer(null);
            finish();
            return;
        }
        current = this;
        setTitle("Verify that it's you");
        if (getActionBar() != null) {
            getActionBar().setDisplayHomeAsUpEnabled(true);
        }
        forget();

        web = new WebView(this);
        WebSettings settings = web.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setSupportMultipleWindows(false);
        settings.setSaveFormData(false);
        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri uri = request.getUrl();
                if (allowed(uri)) {
                    return false;
                }
                // Proton's other links (terms, help) open in the browser.
                if (proton(uri) && request.isForMainFrame()) {
                    try {
                        startActivity(new Intent(Intent.ACTION_VIEW, uri)
                                .addCategory(Intent.CATEGORY_BROWSABLE));
                    } catch (ActivityNotFoundException ignored) {
                    }
                }
                return true;
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                bridge(view, url);
            }
        });

        FrameLayout frame = new FrameLayout(this);
        frame.addView(web);
        // Edge to edge (Android 15 and later): keep the page clear of the
        // navigation bar and the keyboard.
        frame.setOnApplyWindowInsetsListener((v, insets) -> {
            if (Build.VERSION.SDK_INT >= 30) {
                Insets bars = insets.getInsets(
                        WindowInsets.Type.navigationBars() | WindowInsets.Type.displayCutout());
                Insets ime = insets.getInsets(WindowInsets.Type.ime());
                v.setPadding(bars.left, 0, bars.right, Math.max(bars.bottom, ime.bottom));
            }
            return insets;
        });
        setContentView(frame);
        web.loadUrl(url);
    }

    /** Relays the top frame's results to Rust (see the class comment). */
    private void bridge(WebView view, String url) {
        Uri page = Uri.parse(url);
        if (!proton(page)) {
            return;
        }
        byte[] random = new byte[16];
        new SecureRandom().nextBytes(random);
        StringBuilder secret = new StringBuilder();
        for (byte b : random) {
            secret.append(String.format("%02x", b));
        }
        WebMessagePort[] ports = view.createWebMessageChannel();
        port = ports[0];
        port.setWebMessageCallback(new WebMessagePort.WebMessageCallback() {
            @Override
            public void onMessage(WebMessagePort p, WebMessage message) {
                if (message.getData() != null && !answered) {
                    answer(message.getData());
                }
            }
        });
        String key = "picacho-bridge:" + secret;
        String script = "(() => {\n"
                + "  if (window.__picachoBridge) return;\n"
                + "  window.__picachoBridge = true;\n"
                + "  let port = null; const queue = [];\n"
                + "  const send = (m) => {\n"
                + "    const s = typeof m === 'string' ? m : JSON.stringify(m);\n"
                + "    if (port) port.postMessage(s); else queue.push(s);\n"
                + "  };\n"
                + "  const proton = (origin) => {\n"
                + "    try { const u = new URL(origin);\n"
                + "      return u.protocol === 'https:' && (u.hostname === 'proton.me' || u.hostname.endsWith('.proton.me'));\n"
                + "    } catch (_) { return false; }\n"
                + "  };\n"
                + "  window.addEventListener('message', (e) => {\n"
                + "    if (!port && e.data === '" + key + "' && e.ports && e.ports.length) {\n"
                + "      port = e.ports[0]; queue.splice(0).forEach((s) => port.postMessage(s));\n"
                + "      e.stopImmediatePropagation(); return;\n"
                + "    }\n"
                + "    if (proton(e.origin)) send(e.data);\n"
                + "  }, true);\n"
                + "  window.AndroidInterface = { dispatch: send };\n"
                + "})();";
        Uri origin = Uri.parse(page.getScheme() + "://" + page.getHost());
        view.evaluateJavascript(script, ignored ->
                view.postWebMessage(new WebMessage(key, new WebMessagePort[] {ports[1]}), origin));
    }

    private void answer(String value) {
        if (value == null) {
            answered = true;
        }
        Native.answer(id, value);
    }

    /** Like a private window: no cookies or storage from before or after. */
    private static void forget() {
        CookieManager.getInstance().removeAllCookies(null);
        WebStorage.getInstance().deleteAllData();
    }

    @Override
    public boolean onOptionsItemSelected(MenuItem item) {
        if (item.getItemId() == android.R.id.home) {
            finish();
            return true;
        }
        return super.onOptionsItemSelected(item);
    }

    @Override
    protected void onDestroy() {
        if (current == this) {
            current = null;
        }
        if (!answered && isFinishing()) {
            answer(null);
        }
        if (web != null) {
            // Its pages and images from the check too, not only cookies and
            // storage (the cache is shared by the app's web views).
            web.clearCache(true);
            web.clearHistory();
            web.destroy();
            forget();
            CookieManager.getInstance().flush();
        }
        super.onDestroy();
    }
}
