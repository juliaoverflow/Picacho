// Picacho in the Share sheet: text, links, pictures and files shared from
// another app start a new message, as Android's Share target does.
//
// The extension doesn't send mail itself (the mail engine is the app). It
// copies what was shared into the App Group container (Shares/<id>/: the
// files, then share.json naming them, with the subject and text) and opens
// Picacho, which takes the share (Shares.swift) and opens the composer. If
// Picacho can't be opened from here, it takes the share the next time it
// comes to the front.

import UIKit
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    private let status = UILabel()
    private let spinner = UIActivityIndicatorView(style: .medium)
    private var started = false

    /// Picacho's bundle ID: this extension's, without ".share".
    private var appBundle: String {
        let own = Bundle.main.bundleIdentifier ?? "tech.pinal.picacho.share"
        return own.hasSuffix(".share") ? String(own.dropLast(".share".count)) : own
    }

    /// The language Picacho shows, from the widget snapshot it writes into
    /// the App Group (empty before it has written one: the system's).
    private func picachoLanguage() -> String {
        guard
            let container = FileManager.default.containerURL(
                forSecurityApplicationGroupIdentifier: "group.\(appBundle)"),
            let data = try? Data(contentsOf: container.appendingPathComponent("widget-snapshot.json")),
            let snapshot = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return ""
        }
        return snapshot["language"] as? String ?? ""
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        PicachoLanguage.use(picachoLanguage())
        view.backgroundColor = .systemBackground
        status.text = String(localized: "Starting a message in Picacho…", bundle: .picacho)
        status.textAlignment = .center
        status.numberOfLines = 0
        status.font = .preferredFont(forTextStyle: .body)
        status.adjustsFontForContentSizeCategory = true
        spinner.startAnimating()
        let stack = UIStackView(arrangedSubviews: [spinner, status])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            stack.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        Task { await handOver() }
    }

    private func handOver() async {
        let files = FileManager.default
        guard
            let container = files.containerURL(forSecurityApplicationGroupIdentifier: "group.\(appBundle)")
        else {
            return fail(String(localized: "Picacho couldn't take this. Open Picacho once, then share again.", bundle: .picacho))
        }
        let id = UUID().uuidString
        let folder = container
            .appendingPathComponent("Shares", isDirectory: true)
            .appendingPathComponent(id, isDirectory: true)
        do {
            try files.createDirectory(at: folder, withIntermediateDirectories: true)
            // Shared files (copies of mail-to-be) stay on this device: not
            // in iCloud or computer backups of the App Group's container.
            var values = URLResourceValues()
            values.isExcludedFromBackup = true
            var shares = folder.deletingLastPathComponent()
            try? shares.setResourceValues(values)
            var share = folder
            try? share.setResourceValues(values)
        } catch {
            return fail(String(
                localized: "Picacho couldn't take this: \(error.localizedDescription)",
                bundle: .picacho,
                comment: "After the colon: why, from the system"))
        }

        var subject = ""
        var texts: [String] = []
        var names: [String] = []
        let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
        for item in items {
            if subject.isEmpty, let title = item.attributedTitle?.string {
                subject = title.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            if let content = item.attributedContentText?.string.trimmingCharacters(in: .whitespacesAndNewlines),
                !content.isEmpty
            {
                texts.append(content)
            }
            for provider in item.attachments ?? [] {
                switch await Self.load(provider, into: folder, taken: names) {
                case .text(let text) where !text.isEmpty && !texts.contains(text):
                    texts.append(text)
                case .file(let name):
                    names.append(name)
                default:
                    break
                }
            }
        }

        // share.json last: Picacho takes only finished shares.
        let manifest: [String: Any] = ["subject": subject, "text": texts.joined(separator: "\n\n"), "files": names]
        do {
            let data = try JSONSerialization.data(withJSONObject: manifest)
            try data.write(to: folder.appendingPathComponent("share.json"), options: .atomic)
        } catch {
            try? files.removeItem(at: folder)
            return fail(String(
                localized: "Picacho couldn't take this: \(error.localizedDescription)",
                bundle: .picacho,
                comment: "After the colon: why, from the system"))
        }

        let url = URL(string: "\(appBundle)://share/\(id)")!
        if await open(url) {
            extensionContext?.completeRequest(returningItems: nil)
        } else {
            spinner.stopAnimating()
            status.text = String(localized: "Open Picacho to finish your message.", bundle: .picacho)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { [weak self] in
                self?.extensionContext?.completeRequest(returningItems: nil)
            }
        }
    }

    private enum Loaded: Sendable {
        case text(String)
        case file(String)
        case nothing
    }

    /// One shared item: a file (copied into `folder` under a name not in
    /// `taken`), or text (a web link, or plain text).
    nonisolated private static func load(_ provider: NSItemProvider, into folder: URL, taken: [String]) async -> Loaded {
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            guard let url = try? await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? URL
            else { return .nothing }
            return copy(url, named: url.lastPathComponent, into: folder, taken: taken)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            let item = try? await provider.loadItem(forTypeIdentifier: UTType.url.identifier)
            if let url = item as? URL {
                return url.isFileURL ? copy(url, named: url.lastPathComponent, into: folder, taken: taken)
                    : .text(url.absoluteString)
            }
            if let data = item as? Data, let url = URL(dataRepresentation: data, relativeTo: nil) {
                return .text(url.absoluteString)
            }
            return .nothing
        }
        let data = provider.registeredTypeIdentifiers.first { UTType($0)?.conforms(to: .data) == true }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
            data.map({ UTType($0)?.conforms(to: .text) == true }) ?? true
        {
            let item = try? await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
            if let text = item as? String {
                return .text(text)
            }
            if let data = item as? Data {
                return .text(String(decoding: data, as: UTF8.self))
            }
            return .nothing
        }
        guard let type = data else { return .nothing }
        // A picture or other data without a file of its own: iOS writes one
        // for as long as the handler runs.
        return await withCheckedContinuation { done in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
                guard let url else { return done.resume(returning: .nothing) }
                var name = provider.suggestedName ?? url.deletingPathExtension().lastPathComponent
                let ext = url.pathExtension.isEmpty ? (UTType(type)?.preferredFilenameExtension ?? "") : url.pathExtension
                if !ext.isEmpty, (name as NSString).pathExtension.isEmpty {
                    name += "." + ext
                }
                done.resume(returning: copy(url, named: name, into: folder, taken: taken))
            }
        }
    }

    /// The most one share copies, per file and in all: as much as a message
    /// may attach (as Android's shares and picks), so a large share can't
    /// fill the device.
    nonisolated private static let maxFile: Int64 = 100 * 1024 * 1024
    nonisolated private static let maxTotal: Int64 = 100 * 1024 * 1024

    /// Copies the file at `url` into `folder` as `name` (made safe, and
    /// unlike the names `taken`): a regular file only, within `maxFile`, and
    /// with what `folder` holds already within `maxTotal`.
    nonisolated private static func copy(_ url: URL, named name: String, into folder: URL, taken: [String]) -> Loaded {
        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped { url.stopAccessingSecurityScopedResource() }
        }
        guard let source = try? url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
            source.isRegularFile == true
        else { return .nothing }
        let size = Int64(source.fileSize ?? 0)
        let held = ((try? FileManager.default.contentsOfDirectory(
            at: folder, includingPropertiesForKeys: [.fileSizeKey])) ?? [])
            .reduce(Int64(0)) { sum, file in
                sum + Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
            }
        guard size <= maxFile, held + size <= maxTotal else { return .nothing }
        var safe = name.replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "_")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if safe.isEmpty || safe == "." || safe == ".." || safe.hasPrefix(".") {
            safe = "attachment" + (safe.hasPrefix(".") ? safe : "")
        }
        var unique = safe
        var n = 2
        let stem = (safe as NSString).deletingPathExtension
        let ext = (safe as NSString).pathExtension
        while taken.contains(unique) || FileManager.default.fileExists(atPath: folder.appendingPathComponent(unique).path) {
            unique = ext.isEmpty ? "\(stem) \(n)" : "\(stem) \(n).\(ext)"
            n += 1
        }
        do {
            try FileManager.default.copyItem(at: url, to: folder.appendingPathComponent(unique))
            return .file(unique)
        } catch {
            return .nothing
        }
    }

    /// Opens Picacho at `url`. Extensions have no `UIApplication.shared`;
    /// the responder chain still reaches the application object.
    private func open(_ url: URL) async -> Bool {
        let selector = NSSelectorFromString("openURL:options:completionHandler:")
        var responder: UIResponder? = self
        while let next = responder {
            if let application = next as? UIApplication, application.responds(to: selector) {
                typealias Open = @convention(c) (
                    AnyObject, Selector, URL, NSDictionary, (@convention(block) (Bool) -> Void)?
                ) -> Void
                let call = unsafeBitCast(application.method(for: selector), to: Open.self)
                return await withCheckedContinuation { done in
                    call(application, selector, url, NSDictionary()) { opened in
                        done.resume(returning: opened)
                    }
                }
            }
            responder = next.next
        }
        return false
    }

    private func fail(_ message: String) {
        spinner.stopAnimating()
        status.text = message
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            self?.extensionContext?.cancelRequest(
                withError: NSError(domain: "tech.pinal.picacho.share", code: 1))
        }
    }
}
