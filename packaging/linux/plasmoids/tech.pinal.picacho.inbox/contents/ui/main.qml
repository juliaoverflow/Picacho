import QtQuick
import QtQuick.Layouts
import QtCore
import org.kde.plasma.plasmoid
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

PlasmoidItem {
    id: root

    property var snapshot: null
    property int unreadCount: 0
    property string headerTitle: "Inbox"
    property var filteredMessages: []
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
        var r = ((n >> 16) & 255) / 255.0;
        var g = ((n >> 8) & 255) / 255.0;
        var b = (n & 255) / 255.0;
        return Qt.rgba(r, g, b, 1.0);
    }

    function isDarkTheme() {
        var bg = Kirigami.Theme.backgroundColor;
        return (0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b) < 0.5;
    }

    function formatShortDate(epochSec) {
        if (!epochSec) return "";
        var d = new Date(epochSec * 1000);
        var now = new Date();
        if (d.toDateString() === now.toDateString()) {
            return Qt.formatTime(d, "HH:mm");
        }
        var diffDays = (now.getTime() - d.getTime()) / 86400000;
        if (diffDays >= 0 && diffDays < 6) {
            return Qt.formatDate(d, "ddd");
        }
        if (d.getFullYear() === now.getFullYear()) {
            return Qt.formatDate(d, "MMM d");
        }
        return Qt.formatDate(d, "MMM d, yyyy");
    }

    function openUri(uri) {
        Qt.openUrlExternally(uri);
    }

    function inboxUri() {
        var acc = Plasmoid.configuration.accountId;
        return acc ? ("picacho://widget/inbox?account=" + encodeURIComponent(acc)) : "picacho://widget/inbox";
    }

    function composeUri() {
        var acc = Plasmoid.configuration.accountId;
        return acc ? ("picacho://widget/compose?account=" + encodeURIComponent(acc)) : "picacho://widget/compose";
    }

    function useSnapshot(snap) {
        if (!snap) return;
        root.snapshot = snap;
        root.applySnapshot();
    }

    // Reads widget-snapshot.json (written by Picacho whenever its mail
    // changes), or has Picacho build it when it isn't there yet.
    function refreshSnapshot() {
        try {
            if (!execSource) {
                execSource = Qt.createQmlObject(
                    'import org.kde.plasma.plasma5support as P5; P5.DataSource { engine: "executable" }',
                    root,
                    "picachoInboxExec"
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

    function applySnapshot() {
        if (!snapshot) return;
        var accId = Plasmoid.configuration.accountId || "";
        var unreadOnly = !!Plasmoid.configuration.unreadOnly;
        accentColor = rgb24ToColor(isDarkTheme() ? (snapshot.accent_dark || 0x86cfaa) : (snapshot.accent_light || 0x246846));

        var count = snapshot.total_unread || 0;
        var title = "All Inboxes";
        var accs = snapshot.accounts || [];
        if (accId) {
            count = 0;
            title = "Inbox";
            for (var i = 0; i < accs.length; i++) {
                if (accs[i].id === accId) {
                    count = accs[i].unread || 0;
                    title = accs[i].name || accs[i].email || "Inbox";
                    break;
                }
            }
        } else if (accs.length === 1) {
            title = accs[0].name || accs[0].email || "Inbox";
        }
        if (unreadOnly) {
            title = title + " · Unread";
        }
        unreadCount = count;
        headerTitle = title;

        var msgs = snapshot.messages || [];
        var out = [];
        for (var j = 0; j < msgs.length && out.length < 25; j++) {
            var m = msgs[j];
            if (accId && m.account_id !== accId) continue;
            if (unreadOnly && !m.unread) continue;
            out.push(m);
        }
        filteredMessages = out;
    }

    Connections {
        target: Plasmoid.configuration
        function onAccountIdChanged() { root.applySnapshot(); }
        function onUnreadOnlyChanged() { root.applySnapshot(); }
    }

    Timer {
        interval: 15000
        running: true
        repeat: true
        onTriggered: root.refreshSnapshot()
    }

    Component.onCompleted: refreshSnapshot()

    compactRepresentation: MouseArea {
        Layout.minimumWidth: Kirigami.Units.iconSizes.medium
        Layout.minimumHeight: Kirigami.Units.iconSizes.medium
        onClicked: root.expanded = !root.expanded

        RowLayout {
            anchors.centerIn: parent
            spacing: 4
            Kirigami.Icon {
                source: "mail-folder-inbox"
                implicitWidth: Kirigami.Units.iconSizes.smallMedium
                implicitHeight: Kirigami.Units.iconSizes.smallMedium
            }
            Rectangle {
                visible: root.unreadCount > 0
                radius: height / 2
                color: root.accentColor
                implicitWidth: Math.max(height, badgeText.implicitWidth + 8)
                implicitHeight: 16
                PlasmaComponents.Label {
                    id: badgeText
                    anchors.centerIn: parent
                    text: root.unreadCount > 999 ? "999+" : String(root.unreadCount)
                    font.pixelSize: 10
                    font.bold: true
                    color: root.isDarkTheme() ? "#13291e" : "#ffffff"
                }
            }
        }
    }

    fullRepresentation: Item {
        Layout.minimumWidth: Kirigami.Units.gridUnit * 15
        Layout.minimumHeight: Kirigami.Units.gridUnit * 12
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
        Layout.preferredHeight: Kirigami.Units.gridUnit * 22

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing * 2
            spacing: Kirigami.Units.smallSpacing

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                MouseArea {
                    Layout.fillWidth: true
                    implicitHeight: titleRow.implicitHeight
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.openUri(root.inboxUri())

                    RowLayout {
                        id: titleRow
                        anchors.fill: parent
                        spacing: 6
                        Kirigami.Icon {
                            source: "mail-folder-inbox"
                            implicitWidth: 18
                            implicitHeight: 18
                        }
                        PlasmaComponents.Label {
                            text: root.headerTitle
                            font.bold: true
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                        Rectangle {
                            visible: root.unreadCount > 0
                            radius: height / 2
                            color: root.accentColor
                            implicitWidth: Math.max(height, headerBadge.implicitWidth + 10)
                            implicitHeight: 18
                            PlasmaComponents.Label {
                                id: headerBadge
                                anchors.centerIn: parent
                                text: root.unreadCount > 999 ? "999+" : String(root.unreadCount)
                                font.pixelSize: 11
                                font.bold: true
                                color: root.isDarkTheme() ? "#13291e" : "#ffffff"
                            }
                        }
                    }
                }

                PlasmaComponents.ToolButton {
                    icon.name: "view-refresh"
                    display: PlasmaComponents.AbstractButton.IconOnly
                    onClicked: root.refreshSnapshot()
                    PlasmaComponents.ToolTip.text: "Refresh"
                    PlasmaComponents.ToolTip.visible: hovered
                }

                PlasmaComponents.ToolButton {
                    icon.name: "mail-message-new"
                    display: PlasmaComponents.AbstractButton.IconOnly
                    onClicked: root.openUri(root.composeUri())
                    PlasmaComponents.ToolTip.text: "Compose"
                    PlasmaComponents.ToolTip.visible: hovered
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }

            ListView {
                id: msgList
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 2
                model: root.filteredMessages

                delegate: MouseArea {
                    id: rowArea
                    width: msgList.width
                    height: rowContent.implicitHeight + 12
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        var uri = "picacho://widget/message?account="
                            + encodeURIComponent(modelData.account_id)
                            + "&id=" + encodeURIComponent(modelData.id);
                        root.openUri(uri);
                    }

                    Rectangle {
                        anchors.fill: parent
                        radius: 6
                        color: rowArea.containsMouse ? Kirigami.Theme.hoverColor : "transparent"
                        opacity: rowArea.containsMouse ? 0.16 : 0
                    }

                    RowLayout {
                        id: rowContent
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.margins: 6
                        spacing: 8

                        Rectangle {
                            width: 7
                            height: 7
                            radius: 3.5
                            Layout.alignment: Qt.AlignTop
                            Layout.topMargin: 5
                            color: modelData.unread ? root.accentColor : "transparent"
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 1

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 4
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    text: modelData.sender || "(Unknown sender)"
                                    font.bold: !!modelData.unread
                                    opacity: modelData.unread ? 1.0 : 0.78
                                    elide: Text.ElideRight
                                }
                                PlasmaComponents.Label {
                                    text: root.formatShortDate(modelData.date)
                                    font.pixelSize: 11
                                    opacity: 0.65
                                }
                            }

                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                text: modelData.subject || "(No subject)"
                                font.bold: !!modelData.unread
                                opacity: modelData.unread ? 0.95 : 0.78
                                elide: Text.ElideRight
                            }

                            PlasmaComponents.Label {
                                visible: !!(modelData.snippet && modelData.snippet.length > 0)
                                Layout.fillWidth: true
                                text: modelData.snippet || ""
                                font.pixelSize: 11
                                opacity: 0.65
                                elide: Text.ElideRight
                            }
                        }
                    }
                }

                PlasmaComponents.Label {
                    anchors.centerIn: parent
                    visible: msgList.count === 0
                    opacity: 0.65
                    text: (!root.snapshot || !root.snapshot.accounts || root.snapshot.accounts.length === 0)
                        ? "Open Picacho to add an account"
                        : (Plasmoid.configuration.unreadOnly ? "All caught up" : "No messages in Inbox")
                }
            }
        }
    }
}
