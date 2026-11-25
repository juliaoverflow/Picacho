// Built by scripts/bundle.sh into
// Picacho.app/Contents/Frameworks/libpicacho_widget_reload.dylib, which
// Picacho loads (`src/manage/widgets.rs`) after writing widget-snapshot.json
// so its macOS Desktop and Notification Center widgets refresh at once.
// WidgetKit reloads the widgets of the app that asks, so this runs inside
// Picacho's own process rather than as a helper of its own.
import WidgetKit

@_cdecl("picacho_widgets_reload")
public func picachoWidgetsReload() {
    WidgetCenter.shared.reloadAllTimelines()
}
