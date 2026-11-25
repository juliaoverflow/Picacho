// The host app's moments (main.m), for the Swift side.

import Foundation

/// The app is launching (in front, or in the background for a refresh or a
/// notification's button): before it finishes, the notification center's
/// delegate and the background refresh's handler.
@_cdecl("picacho_ios_app_launched")
public func picachoIosAppLaunched() {
    PicachoNotifications.install()
    PicachoBackground.register()
}

/// The window is up: asks to notify (the first time), and takes a share
/// that was waiting.
@_cdecl("picacho_ios_scene_started")
public func picachoIosSceneStarted() {
    PicachoNotifications.requestPermission()
    PicachoShares.collect()
}

/// Picacho left the screen: asks to be woken to check for mail.
@_cdecl("picacho_ios_scene_backgrounded")
public func picachoIosSceneBackgrounded() {
    PicachoBackground.schedule()
}
