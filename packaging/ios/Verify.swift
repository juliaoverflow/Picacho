// Proton's human verification (its CAPTCHA, or a code by email or text) in
// a WKWebView, in a sheet over Picacho, for Rust (src/ios.rs `verify`;
// android's VerifyActivity and src/display/verify.rs on desktops do the
// same).
//
// Proton's page reports its result with postMessage and, in Proton's own
// apps, through a native bridge (`window.webkit.messageHandlers.iOS`, and
// `window.AndroidInterface`). A script in the page's top frame relays posted
// messages from Proton's own origins only (the CAPTCHA frames Proton embeds,
// hCaptcha's and Google's, can post too), and only messages from the top
// frame, on a Proton origin, reach Rust, which keeps only the result. The
// page goes only to Proton and those CAPTCHA services, and nothing it
// stores stays (a non-persistent data store).

import UIKit
import WebKit

final class PicachoVerify: UIViewController, WKNavigationDelegate, WKScriptMessageHandler,
    UIAdaptivePresentationControllerDelegate
{
    static let page = "https://verify.proton.me/"

    /// The page showing, for Rust to close. Main thread only.
    private static var current: PicachoVerify?

    private let id: Int32
    private let url: URL
    private var answered = false
    private var web: WKWebView?

    /// Shows `url` for request `id`; false if it can't be.
    static func start(id: Int32, url: String) -> Bool {
        guard url.hasPrefix(page), let url = URL(string: url), let top = picachoTopController() else {
            return false
        }
        current?.finish()
        let verify = PicachoVerify(id: id, url: url)
        let navigation = UINavigationController(rootViewController: verify)
        navigation.presentationController?.delegate = verify
        top.present(navigation, animated: true)
        current = verify
        return true
    }

    /// Rust has what it waited for, or gave up.
    static func close(id: Int32) {
        guard let verify = current, verify.id == id else { return }
        verify.answered = true
        verify.finish()
    }

    private init(id: Int32, url: URL) {
        self.id = id
        self.url = url
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override func loadView() {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        let content = WKUserContentController()
        content.addUserScript(
            WKUserScript(source: Self.bridge, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let relay = Relay(self)
        content.add(relay, name: "picacho")
        content.add(relay, name: "iOS")
        configuration.userContentController = content
        let web = WKWebView(frame: .zero, configuration: configuration)
        web.navigationDelegate = self
        self.web = web
        view = web
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Verify that it's you"
        navigationItem.leftBarButtonItem = UIBarButtonItem(
            systemItem: .cancel,
            primaryAction: UIAction { [weak self] _ in self?.finish() })
        web?.load(URLRequest(url: url))
    }

    /// Relays the top frame's posted messages, and Proton's native bridges,
    /// to `window.webkit.messageHandlers.picacho`.
    private static let bridge = """
        (() => {
          if (window.__picachoBridge) return;
          window.__picachoBridge = true;
          const send = (m) => {
            try {
              window.webkit.messageHandlers.picacho.postMessage(typeof m === "string" ? m : JSON.stringify(m));
            } catch (_) {}
          };
          const proton = (origin) => {
            try {
              const u = new URL(origin);
              return u.protocol === "https:" && (u.hostname === "proton.me" || u.hostname.endsWith(".proton.me"));
            } catch (_) { return false; }
          };
          window.addEventListener("message", (e) => { if (proton(e.origin)) send(e.data); }, true);
          window.AndroidInterface = { dispatch: send };
        })();
        """

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        let origin = message.frameInfo.securityOrigin
        guard !answered, message.frameInfo.isMainFrame, origin.protocol == "https", Self.proton(origin.host)
        else { return }
        let text: String
        if let body = message.body as? String {
            text = body
        } else if JSONSerialization.isValidJSONObject(message.body),
            let data = try? JSONSerialization.data(withJSONObject: message.body)
        {
            text = String(decoding: data, as: UTF8.self)
        } else {
            return
        }
        answer(text)
    }

    // The async form: its signature is the same in every SDK (the
    // completion handler's attributes changed), so it's always the one
    // WebKit calls.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async
        -> WKNavigationActionPolicy
    {
        guard let url = action.request.url else { return .cancel }
        if Self.allowed(url) {
            return .allow
        }
        // Proton's other links (terms, help) open in Safari.
        if url.scheme == "https", Self.proton(url.host ?? ""), action.targetFrame?.isMainFrame ?? true {
            UIApplication.shared.open(url, options: [:], completionHandler: nil)
        }
        return .cancel
    }

    /// The page may only go to Proton and the CAPTCHA services it embeds.
    static func allowed(_ url: URL) -> Bool {
        if url.absoluteString == "about:blank" {
            return true
        }
        guard url.scheme == "https", let host = url.host?.lowercased() else { return false }
        let domains = ["proton.me", "protonmail.com", "protonmail.ch", "hcaptcha.com", "recaptcha.net"]
        if domains.contains(where: { under(host, $0) }) {
            return true
        }
        return (host == "www.google.com" && url.path.hasPrefix("/recaptcha/")) || host == "www.gstatic.com"
    }

    private static func proton(_ host: String) -> Bool {
        let host = host.lowercased()
        return under(host, "proton.me") || under(host, "protonmail.com")
    }

    private static func under(_ host: String, _ domain: String) -> Bool {
        host == domain || host.hasSuffix("." + domain)
    }

    /// Swiped away.
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        closed()
    }

    private func answer(_ value: String) {
        withPicachoUTF8(value) { picacho_ios_answer(id, $0, $1) }
    }

    /// Ends the page: dismissed, and Rust told there's nothing more.
    private func finish() {
        closed()
        presentingViewController?.dismiss(animated: true)
    }

    private func closed() {
        if Self.current === self {
            Self.current = nil
        }
        if !answered {
            answered = true
            picacho_ios_answer(id, nil, 0)
        }
        if let web {
            web.stopLoading()
            web.configuration.userContentController.removeAllScriptMessageHandlers()
            web.navigationDelegate = nil
        }
    }

    /// Hands messages to the page's controller without the web view's
    /// content controller keeping it alive.
    private final class Relay: NSObject, WKScriptMessageHandler {
        private weak var target: PicachoVerify?
        init(_ target: PicachoVerify) {
            self.target = target
        }
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            target?.userContentController(controller, didReceive: message)
        }
    }
}

@_cdecl("picacho_ios_verify_start")
public func picachoIosVerifyStart(_ id: Int32, _ url: UnsafePointer<UInt8>?, _ len: Int) -> Bool {
    let url = picachoString(url, len)
    return picachoOnMain { PicachoVerify.start(id: id, url: url) }
}

@_cdecl("picacho_ios_verify_close")
public func picachoIosVerifyClose(_ id: Int32) {
    DispatchQueue.main.async { PicachoVerify.close(id: id) }
}
