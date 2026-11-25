// Helpers for the Swift side's calls to and from the Rust library
// (src/ios.rs): strings as UTF-8 bytes and a length, JSON, and completions
// Rust calls once it's done.

import Foundation
import UIKit

/// `len` UTF-8 bytes at `ptr`, as a string (empty for none).
func picachoString(_ ptr: UnsafePointer<UInt8>?, _ len: Int) -> String {
    guard let ptr, len > 0 else { return "" }
    return String(decoding: UnsafeBufferPointer(start: ptr, count: len), as: UTF8.self)
}

/// JSON Rust passed as `len` UTF-8 bytes at `ptr`.
func picachoJSON(_ ptr: UnsafePointer<UInt8>?, _ len: Int) -> Any? {
    guard let ptr, len > 0 else { return nil }
    return try? JSONSerialization.jsonObject(with: Data(bytes: ptr, count: len))
}

/// Calls `body` with `text`'s UTF-8 bytes and their length.
func withPicachoUTF8<R>(_ text: String, _ body: (UnsafePointer<UInt8>?, Int) -> R) -> R {
    var text = text
    return text.withUTF8 { body($0.baseAddress, $0.count) }
}

/// Work to do once Rust says it's done: handed to Rust retained, as a
/// `PicachoDone` context (`picachoDone` takes it back).
final class PicachoCompletion {
    private let body: (Bool) -> Void
    init(_ body: @escaping (Bool) -> Void) {
        self.body = body
    }
    /// The context to pass with `picachoDone`; Rust calls it exactly once.
    func retained() -> UnsafeMutableRawPointer {
        Unmanaged.passRetained(self).toOpaque()
    }
    fileprivate func run(_ ok: Bool) {
        body(ok)
    }
}

/// The `PicachoDone` Rust calls: runs the completion on the main queue.
let picachoDone: PicachoDone = { context, ok in
    guard let context else { return }
    let completion = Unmanaged<PicachoCompletion>.fromOpaque(context).takeRetainedValue()
    DispatchQueue.main.async { completion.run(ok) }
}

/// Runs `body` on the main thread and returns what it returns: at once on
/// it, otherwise waiting for it (Rust calls from its own threads).
func picachoOnMain<R: Sendable>(_ body: @MainActor () -> R) -> R {
    if Thread.isMainThread {
        return MainActor.assumeIsolated(body)
    }
    return DispatchQueue.main.sync { MainActor.assumeIsolated(body) }
}

/// The view controller in front, to present a sheet over.
@MainActor
func picachoTopController() -> UIViewController? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let windows = scenes.flatMap(\.windows)
    var top = (windows.first { $0.isKeyWindow } ?? windows.first)?.rootViewController
    while let presented = top?.presentedViewController, !presented.isBeingDismissed {
        top = presented
    }
    return top
}

/// The App Group container Picacho shares with its extensions:
/// "group." and the app's bundle ID.
func picachoGroupContainer() -> URL? {
    guard let bundle = Bundle.main.bundleIdentifier, !bundle.isEmpty else { return nil }
    return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.\(bundle)")
}
