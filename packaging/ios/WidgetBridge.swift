// Bridges Rust's widget snapshot publisher (`src/ios.rs`) to the shared
// App Group container and WidgetKit's timeline reloader.

import Foundation
import WidgetKit

@_cdecl("picacho_ios_publish_widgets")
public func picachoIosPublishWidgets(_ ptr: UnsafePointer<UInt8>?, _ len: Int) {
    guard let ptr, len > 0 else { return }
    let data = Data(bytes: ptr, count: len)
    guard let bundleId = Bundle.main.bundleIdentifier, !bundleId.isEmpty else { return }
    let groupId = "group.\(bundleId)"
    guard let container = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: groupId
    ) else {
        return
    }
    var fileUrl = container.appendingPathComponent("widget-snapshot.json")
    do {
        try data.write(to: fileUrl, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? fileUrl.setResourceValues(values)
    } catch {
        NSLog("Picacho: could not write widget snapshot: %@", error.localizedDescription)
        return
    }
    WidgetCenter.shared.reloadAllTimelines()
}
