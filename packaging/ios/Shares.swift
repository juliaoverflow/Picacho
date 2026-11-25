// What the Share extension handed Picacho (ShareExtension/ShareViewController.swift):
// each share is a folder in the App Group container's Shares/, with its files
// and, written last, share.json (subject, text, file names). Picacho takes
// them when the extension opens it, or whenever it comes to the front (if the
// extension couldn't open it). The files move into Picacho's own caches, and
// each share becomes requests for Rust's window (src/display/launch.rs), as
// Android's Share target makes them (Shared.java): a `mailto:` with the
// subject and text, then `attach:` and each file's path, which start one
// new message.

import Foundation

enum PicachoShares {
    /// The longest request Rust keeps (launch::MAX_REQUEST is 8 KiB).
    private static let most = 8000
    /// Shares left half-written (the extension ended) go after a day.
    private static let stale: TimeInterval = 24 * 60 * 60

    /// Hands every finished share to the window.
    static func collect() {
        let files = FileManager.default
        guard let inbox = picachoGroupContainer()?.appendingPathComponent("Shares", isDirectory: true),
            let shares = try? files.contentsOfDirectory(
                at: inbox, includingPropertiesForKeys: [.creationDateKey], options: [.skipsHiddenFiles])
        else {
            return
        }
        let created = { (url: URL) in
            (try? url.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
        }
        for share in shares.sorted(by: { created($0) < created($1) }) {
            let manifest = share.appendingPathComponent("share.json")
            guard let data = try? Data(contentsOf: manifest),
                let shared = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else {
                if created(share).timeIntervalSinceNow < -stale {
                    try? files.removeItem(at: share)
                }
                continue
            }
            take(shared, from: share)
            try? files.removeItem(at: share)
        }
    }

    private static func take(_ shared: [String: Any], from folder: URL) {
        let files = FileManager.default
        var requests = [mailto(subject: shared["subject"] as? String ?? "", text: shared["text"] as? String ?? "")]
        let caches = files.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("shared", isDirectory: true)
            .appendingPathComponent(folder.lastPathComponent, isDirectory: true)
        try? files.createDirectory(at: caches, withIntermediateDirectories: true)
        for name in shared["files"] as? [String] ?? [] {
            // A name, not a path: nothing outside the share's folder.
            guard !name.isEmpty, !name.contains("/"), name != ".", name != ".." else { continue }
            let from = folder.appendingPathComponent(name)
            let to = caches.appendingPathComponent(name)
            try? files.removeItem(at: to)
            if (try? files.moveItem(at: from, to: to)) != nil {
                requests.append("attach:" + to.path)
            }
        }
        for request in requests {
            withPicachoUTF8(request) { picacho_ios_launch($0, $1) }
        }
    }

    /// A `mailto:` without addresses, with the subject and text (RFC 6068),
    /// the text shortened to fit.
    static func mailto(subject: String, text: String) -> String {
        // Unreserved characters stay; everything else is percent-encoded.
        let allowed = CharacterSet(
            charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        let encode = { (s: String) in s.addingPercentEncoding(withAllowedCharacters: allowed) ?? "" }
        var body = String(text.prefix(6000))
        while true {
            var fields: [String] = []
            if !subject.isEmpty {
                fields.append("subject=" + encode(String(subject.prefix(500))))
            }
            if !body.isEmpty {
                fields.append("body=" + encode(body))
            }
            let mailto = "mailto:" + (fields.isEmpty ? "" : "?" + fields.joined(separator: "&"))
            if mailto.utf8.count <= most || body.isEmpty {
                return mailto
            }
            body = String(body.prefix(body.count * 4 / 5))
        }
    }
}

/// Picacho was opened for a share, or came to the front: takes what the
/// Share extension left.
@_cdecl("picacho_ios_collect_shares")
public func picachoIosCollectShares() {
    PicachoShares.collect()
}
