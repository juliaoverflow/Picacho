// Picacho's notifications on iOS, for Rust (src/ios.rs `notify`;
// src/display/notify.rs decides what they say): new mail, mail back from
// snooze, scheduled mail sent or not, notices from the mail provider and
// event reminders.
//
// Each carries its target (notify::Target's JSON) in its userInfo. Tapping
// one opens Picacho there; its buttons (Mark as Read, Archive, Delete for
// one message or all of them; Reply for one) come from its category, whose
// actions Rust names (`register_categories`). Reply opens Picacho to write;
// the others act without opening it, and iOS keeps Picacho awake until Rust
// says it's done.

import Foundation
import UserNotifications

final class PicachoNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PicachoNotifications()

    /// Becomes the notification center's delegate. Called before the app
    /// finishes launching, so a tap that launched it is heard.
    static func install() {
        UNUserNotificationCenter.current().delegate = shared
    }

    /// Asks whether Picacho may notify, the first time (iOS keeps the
    /// answer; Settings → Notifications → Picacho changes it), as Android
    /// asks when the app opens.
    static func requestPermission() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, error in
            if let error {
                NSLog("Picacho: notifications weren't allowed: %@", error.localizedDescription)
            }
        }
    }

    /// Picacho announces mail only while its window isn't in front (or from
    /// a background refresh), so one that arrives in front still shows.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let target = response.notification.request.content.userInfo["target"] as? String ?? ""
        guard !target.isEmpty else {
            completionHandler()
            return
        }
        switch response.actionIdentifier {
        case UNNotificationDefaultActionIdentifier:
            withPicachoUTF8(target) { picacho_ios_notification_tapped($0, $1) }
            completionHandler()
        case UNNotificationDismissActionIdentifier:
            completionHandler()
        default:
            let action = response.actionIdentifier
            let completion = PicachoCompletion { _ in completionHandler() }
            withPicachoUTF8(target) { target, targetLen in
                withPicachoUTF8(action) { action, actionLen in
                    picacho_ios_notification_action(
                        target, targetLen, action, actionLen, picachoDone, completion.retained())
                }
            }
        }
    }
}

/// Shows a notification Rust described: `id` (one with the same id
/// replaces it), `title`, `subtitle` (the account), `body`, `target` (where
/// a tap goes; empty for nowhere), `category` (its buttons) and `thread`
/// (what it's grouped with).
@_cdecl("picacho_ios_notify")
public func picachoIosNotify(_ ptr: UnsafePointer<UInt8>?, _ len: Int) {
    guard let note = picachoJSON(ptr, len) as? [String: Any] else { return }
    let text = { (key: String) in note[key] as? String ?? "" }
    let content = UNMutableNotificationContent()
    content.title = text("title")
    content.subtitle = text("subtitle")
    content.body = text("body")
    content.sound = .default
    content.threadIdentifier = text("thread")
    content.categoryIdentifier = text("category")
    if !text("target").isEmpty {
        content.userInfo = ["target": text("target")]
    }
    let id = text("id").isEmpty ? UUID().uuidString : text("id")
    let request = UNNotificationRequest(identifier: id, content: content, trigger: nil)
    UNUserNotificationCenter.current().add(request) { error in
        if let error {
            NSLog("Picacho: a notification couldn't be shown: %@", error.localizedDescription)
        }
    }
}

/// The notification categories Rust named, each with its buttons: their
/// action keys and labels, whether they delete, and whether they open
/// Picacho.
@_cdecl("picacho_ios_notification_categories")
public func picachoIosNotificationCategories(_ ptr: UnsafePointer<UInt8>?, _ len: Int) {
    guard let list = picachoJSON(ptr, len) as? [[String: Any]] else { return }
    let categories = list.compactMap { category -> UNNotificationCategory? in
        guard let id = category["id"] as? String else { return nil }
        let actions = (category["actions"] as? [[String: Any]] ?? []).compactMap { action -> UNNotificationAction? in
            guard let key = action["id"] as? String, let title = action["title"] as? String else { return nil }
            // Changing mail from the Lock Screen needs the device unlocked
            // first, as Mail's buttons do.
            var options: UNNotificationActionOptions = [.authenticationRequired]
            if action["destructive"] as? Bool == true {
                options.insert(.destructive)
            }
            if action["foreground"] as? Bool == true {
                options.insert(.foreground)
            }
            return UNNotificationAction(identifier: key, title: title, options: options)
        }
        return UNNotificationCategory(identifier: id, actions: actions, intentIdentifiers: [], options: [])
    }
    UNUserNotificationCenter.current().setNotificationCategories(Set(categories))
}
