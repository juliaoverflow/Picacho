// Checking for mail while Picacho isn't open: background app refresh
// (BGTaskScheduler), for Rust (src/ios.rs `background_refresh`).
//
// iOS doesn't let apps keep their connections open once they leave the
// screen. While checking in the background is on (Settings → General),
// Picacho asks to be woken from time to time; when iOS grants a moment, Rust
// checks each inbox once, sends scheduled mail that's due, brings back
// snoozed mail and announces what's new. iOS decides when, by how Picacho is
// used, and never while Background App Refresh is off for it.

import BackgroundTasks
import Foundation

enum PicachoBackground {
    /// The task's identifier, as Info.plist's BGTaskSchedulerPermittedIdentifiers
    /// lists it: the app's bundle ID and ".refresh".
    static var identifier: String {
        (Bundle.main.bundleIdentifier ?? "tech.pinal.picacho") + ".refresh"
    }

    /// How long after a refresh (or leaving the screen) Picacho asks to be
    /// woken at the earliest; iOS decides when it is.
    static let interval: TimeInterval = 15 * 60

    /// Registers the refresh's handler. BGTaskScheduler wants it before the
    /// app finishes launching.
    static func register() {
        let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
            guard let task = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            run(task)
        }
        if !registered {
            NSLog("Picacho: background refresh isn't permitted (BGTaskSchedulerPermittedIdentifiers).")
        }
    }

    private static func run(_ task: BGAppRefreshTask) {
        // The next one is asked for first, so one cut short has another after it.
        schedule()
        let once = Once()
        task.expirationHandler = {
            picacho_ios_background_expire()
            DispatchQueue.main.async {
                once.run { task.setTaskCompleted(success: false) }
            }
        }
        let completion = PicachoCompletion { ok in
            once.run { task.setTaskCompleted(success: ok) }
        }
        picacho_ios_background_refresh(picachoDone, completion.retained())
    }

    /// Asks to be woken, if checking in the background is on (and stops
    /// asking if it's off). Called as Picacho leaves the screen.
    static func schedule() {
        if picacho_ios_background_enabled() {
            submit()
        } else {
            cancel()
        }
    }

    static func submit() {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: interval)
        do {
            try BGTaskScheduler.shared.submit(request)
        } catch {
            // The Simulator, or Background App Refresh is off.
            NSLog("Picacho: background refresh wasn't scheduled: %@", error.localizedDescription)
        }
    }

    static func cancel() {
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
    }

    /// Runs its work once, on the main thread.
    private final class Once {
        private var done = false
        func run(_ body: () -> Void) {
            guard !done else { return }
            done = true
            body()
        }
    }
}

/// Settings turned checking in the background on or off (src/ios.rs
/// `set_background`).
@_cdecl("picacho_ios_set_background")
public func picachoIosSetBackground(_ on: Bool) {
    DispatchQueue.main.async {
        if on {
            PicachoBackground.submit()
        } else {
            PicachoBackground.cancel()
        }
    }
}
