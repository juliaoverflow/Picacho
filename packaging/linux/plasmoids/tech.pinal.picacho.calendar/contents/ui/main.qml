import QtQuick
import QtQuick.Layouts
import QtCore
import org.kde.plasma.plasmoid
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

PlasmoidItem {
    id: root

    property var snapshot: null

    // The widget's words in the language Picacho shows (the snapshot's
    // labels, which Picacho writes in it), else in English. %1 and %2
    // stand for a and b.
    function word(name, english, a, b) {
        var labels = root.snapshot && root.snapshot.labels;
        var text = (labels && labels[name]) || english;
        return text.replace("%1", function () { return a; }).replace("%2", function () { return b; });
    }
    property var events: []
    property bool calendarConnected: false
    property color accentColor: Kirigami.Theme.highlightColor
    property var execSource: null

    preferredRepresentation: Plasmoid.formFactor === 2 || Plasmoid.formFactor === 3
        ? compactRepresentation
        : fullRepresentation

    // Qt doesn't let QML's XMLHttpRequest read local files (unless
    // QML_XHR_ALLOW_FILE_READ is set), so the snapshot is read through
    // Plasma's executable engine instead.
    function snapshotPath() {
        var base = StandardPaths.writableLocation(StandardPaths.GenericDataLocation).toString();
        if (base.startsWith("file://")) base = decodeURIComponent(base.substring(7));
        return base + "/picacho/widget-snapshot.json";
    }

    function shellQuote(s) {
        return "'" + s.replace(/'/g, "'\\''") + "'";
    }

    function rgb24ToColor(n) {
        return Qt.rgba(((n >> 16) & 255) / 255.0, ((n >> 8) & 255) / 255.0, (n & 255) / 255.0, 1.0);
    }

    function isDarkTheme() {
        var bg = Kirigami.Theme.backgroundColor;
        return (0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b) < 0.5;
    }

    function dayLabel(dateObj) {
        var today = new Date();
        var tomorrow = new Date(today.getFullYear(), today.getMonth(), today.getDate() + 1);
        if (dateObj.toDateString() === today.toDateString()) return word("today", "Today");
        if (dateObj.toDateString() === tomorrow.toDateString()) return word("tomorrow", "Tomorrow");
        return Qt.formatDate(dateObj, "ddd, MMM d");
    }

    function formatEventTime(ev) {
        if (ev.all_day) {
            var utc = new Date(ev.start * 1000);
            var local = new Date(utc.getUTCFullYear(), utc.getUTCMonth(), utc.getUTCDate());
            return dayLabel(local) + " · " + word("all-day", "All day");
        }
        var start = new Date(ev.start * 1000);
        var label = dayLabel(start) + " · " + Qt.formatTime(start, Qt.locale().timeFormat(Locale.ShortFormat));
        if (ev.end && ev.end > ev.start) {
            var end = new Date(ev.end * 1000);
            label += " – " + Qt.formatTime(end, Qt.locale().timeFormat(Locale.ShortFormat));
        }
        return label;
    }

    function applySnapshot(snap) {
        root.snapshot = snap;
        root.accentColor = rgb24ToColor(isDarkTheme() ? (snap.accent_dark || 0x86cfaa) : (snap.accent_light || 0x246846));
        root.calendarConnected = !!snap.calendar_connected;
        root.events = snap.events || [];
    }

    function useSnapshot(snap) {
        if (!snap) return;
        root.applySnapshot(snap);
    }

    // Reads widget-snapshot.json (written by Picacho whenever its mail
    // changes), or has Picacho build it when it isn't there yet.
    function refreshSnapshot() {
        try {
            if (!execSource) {
                execSource = Qt.createQmlObject(
                    'import org.kde.plasma.plasma5support as P5; P5.DataSource { engine: "executable" }',
                    root,
                    "picachoCalendarExec"
                );
                execSource.newData.connect(function(sourceName, data) {
                    execSource.disconnectSource(sourceName);
                    if (data && data.stdout) {
                        try {
                            root.useSnapshot(JSON.parse(data.stdout));
                        } catch (e) {}
                    }
                });
            }
            execSource.connectSource(
                "cat " + shellQuote(snapshotPath()) + " 2>/dev/null || picacho --widget-json 2>/dev/null"
            );
        } catch (e) {}
    }

    Timer {
        interval: 30000
        running: true
        repeat: true
        onTriggered: root.refreshSnapshot()
    }

    Component.onCompleted: refreshSnapshot()

    compactRepresentation: MouseArea {
        Layout.minimumWidth: Kirigami.Units.iconSizes.medium
        Layout.minimumHeight: Kirigami.Units.iconSizes.medium
        onClicked: root.expanded = !root.expanded
        Kirigami.Icon {
            anchors.centerIn: parent
            source: "view-calendar"
            implicitWidth: Kirigami.Units.iconSizes.smallMedium
            implicitHeight: Kirigami.Units.iconSizes.smallMedium
        }
    }

    fullRepresentation: Item {
        Layout.minimumWidth: Kirigami.Units.gridUnit * 14
        Layout.minimumHeight: Kirigami.Units.gridUnit * 10
        Layout.preferredWidth: Kirigami.Units.gridUnit * 19
        Layout.preferredHeight: Kirigami.Units.gridUnit * 18

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing * 2
            spacing: Kirigami.Units.smallSpacing

            RowLayout {
                Layout.fillWidth: true
                spacing: 6

                MouseArea {
                    Layout.fillWidth: true
                    implicitHeight: headerRow.implicitHeight
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Qt.openUrlExternally("picacho://widget/calendar")

                    RowLayout {
                        id: headerRow
                        anchors.fill: parent
                        spacing: 6
                        Kirigami.Icon {
                            source: "view-calendar"
                            implicitWidth: 18
                            implicitHeight: 18
                        }
                        PlasmaComponents.Label {
                            text: word("calendar", "Calendar")
                            font.bold: true
                            Layout.fillWidth: true
                        }
                        PlasmaComponents.Label {
                            text: Qt.formatDate(new Date(), "ddd, MMM d")
                            font.pixelSize: 11
                            opacity: 0.65
                        }
                    }
                }

                PlasmaComponents.ToolButton {
                    icon.name: "view-refresh"
                    display: PlasmaComponents.AbstractButton.IconOnly
                    onClicked: root.refreshSnapshot()
                    PlasmaComponents.ToolTip.text: word("refresh", "Refresh")
                    PlasmaComponents.ToolTip.visible: hovered
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            ListView {
                id: eventList
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 4
                model: root.events

                delegate: MouseArea {
                    id: evArea
                    width: eventList.width
                    height: evContent.implicitHeight + 10
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Qt.openUrlExternally("picacho://widget/calendar")

                    Rectangle {
                        anchors.fill: parent
                        radius: 6
                        color: evArea.containsMouse ? Kirigami.Theme.hoverColor : "transparent"
                        opacity: evArea.containsMouse ? 0.16 : 0
                    }

                    RowLayout {
                        id: evContent
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.margins: 5
                        spacing: 8

                        Rectangle {
                            width: 3
                            Layout.fillHeight: true
                            Layout.minimumHeight: 28
                            radius: 1.5
                            color: (modelData.calendar_color && modelData.calendar_color.length === 7)
                                ? modelData.calendar_color
                                : root.accentColor
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 1
                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                text: modelData.title || word("no-title", "(No title)")
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                text: root.formatEventTime(modelData)
                                font.pixelSize: 11
                                opacity: 0.78
                                elide: Text.ElideRight
                            }
                            PlasmaComponents.Label {
                                visible: !!(modelData.location && modelData.location.length > 0)
                                Layout.fillWidth: true
                                text: modelData.location || ""
                                font.pixelSize: 11
                                opacity: 0.62
                                elide: Text.ElideRight
                            }
                        }
                    }
                }

                PlasmaComponents.Label {
                    anchors.centerIn: parent
                    visible: eventList.count === 0
                    opacity: 0.65
                    text: root.calendarConnected ? word("no-upcoming-events", "No upcoming events") : word("connect-calendar", "Connect a calendar in Settings")
                }
            }
        }
    }
}
