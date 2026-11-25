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
    property var packages: []
    // How many the Summary lists (for "View all"), most of them in the snapshot.
    property int totalCount: 0
    property string headerTitle: word("packages", "Packages")
    property bool showAccountLabel: false
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

    function packagesUri() {
        var acc = Plasmoid.configuration.accountId || "";
        return acc ? ("picacho://widget/packages?account=" + encodeURIComponent(acc)) : "picacho://widget/packages";
    }

    function rgb24ToColor(n) {
        return Qt.rgba(((n >> 16) & 255) / 255.0, ((n >> 8) & 255) / 255.0, (n & 255) / 255.0, 1.0);
    }

    function isDarkTheme() {
        var bg = Kirigami.Theme.backgroundColor;
        return (0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b) < 0.5;
    }

    function formatShortDate(epochSec) {
        if (!epochSec) return "";
        var d = new Date(epochSec * 1000);
        var now = new Date();
        if (d.getFullYear() === now.getFullYear()) return Qt.formatDate(d, "MMM d");
        return Qt.formatDate(d, "MMM d, yyyy");
    }

    function detailText(pkg) {
        var parts = [];
        if (pkg.status) parts.push(pkg.status);
        if (pkg.carrier) parts.push(pkg.carrier);
        if (pkg.expected) parts.push(word("expected", "Expected %1", pkg.expected));
        else {
            var d = formatShortDate(pkg.updated);
            if (d) parts.push(d);
        }
        if (root.showAccountLabel && pkg.account_label) parts.push(pkg.account_label);
        return parts.join(" · ");
    }

    function applySnapshot() {
        if (!snapshot) return;
        var accId = Plasmoid.configuration.accountId || "";
        var accs = snapshot.accounts || [];
        root.accentColor = rgb24ToColor(isDarkTheme() ? (snapshot.accent_dark || 0x86cfaa) : (snapshot.accent_light || 0x246846));

        var title = word("packages", "Packages");
        if (accId) {
            for (var i = 0; i < accs.length; i++) {
                if (accs[i].id === accId) {
                    var label = accs[i].name || accs[i].email || "";
                    if (label) title = word("packages-of", "Packages · %1", label);
                    break;
                }
            }
        }
        root.headerTitle = title;
        root.showAccountLabel = !accId && accs.length > 1;

        // All of them: the list shows as many as fit.
        var all = snapshot.packages || [];
        var out = [];
        for (var j = 0; j < all.length; j++) {
            if (accId && all[j].account_id !== accId) continue;
            out.push(all[j]);
        }
        root.packages = out;
        var total = 0;
        for (var k = 0; k < accs.length; k++) {
            if (!accId || accs[k].id === accId) total += accs[k].packages || 0;
        }
        root.totalCount = Math.max(total, out.length);
    }

    function useSnapshot(snap) {
        if (!snap) return;
        if ((snap.version || 1) < 2) return;
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
                    "picachoPackagesExec"
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

    Connections {
        target: Plasmoid.configuration
        function onAccountIdChanged() { root.applySnapshot(); }
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
            source: "package-x-generic"
            implicitWidth: Kirigami.Units.iconSizes.smallMedium
            implicitHeight: Kirigami.Units.iconSizes.smallMedium
        }
    }

    fullRepresentation: Item {
        Layout.minimumWidth: Kirigami.Units.gridUnit * 15
        Layout.minimumHeight: Kirigami.Units.gridUnit * 11
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
        Layout.preferredHeight: Kirigami.Units.gridUnit * 20

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
                    onClicked: Qt.openUrlExternally(root.packagesUri())

                    RowLayout {
                        id: headerRow
                        anchors.fill: parent
                        spacing: 6
                        Kirigami.Icon {
                            source: "package-x-generic"
                            implicitWidth: 18
                            implicitHeight: 18
                        }
                        PlasmaComponents.Label {
                            text: root.headerTitle
                            font.bold: true
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                    }
                }

                PlasmaComponents.ToolButton {
                    visible: root.totalCount > 0
                    text: word("view-all:" + root.totalCount, "View all %1", root.totalCount)
                    flat: true
                    onClicked: Qt.openUrlExternally(root.packagesUri())
                    PlasmaComponents.ToolTip.text: word("open-in-summary", "Open them in Picacho's Summary")
                    PlasmaComponents.ToolTip.visible: hovered
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

            // Measures a row (as the delegate lays one out), so the list
            // shows as many whole rows as the widget's height fits: no
            // scrolling, no scaling; "View all" has the rest.
            ColumnLayout {
                id: rowSizer
                visible: false
                spacing: 3
                PlasmaComponents.Label { text: "Package"; font.bold: true }
                PlasmaComponents.Label { text: "Shipped"; font.pixelSize: 11 }
                Item { implicitHeight: 3 }
            }

            ListView {
                id: pkgList
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                interactive: false
                readonly property real rowHeight: rowSizer.implicitHeight + 12
                readonly property int fitCount: Math.max(0, Math.floor((height + spacing) / (rowHeight + spacing)))
                spacing: 4
                model: root.packages.slice(0, fitCount)

                delegate: MouseArea {
                    id: pkgArea
                    // The package, for the progress Repeater below, whose own
                    // modelData (0..4) would otherwise shadow it.
                    property var pkg: modelData
                    width: pkgList.width
                    height: pkgList.rowHeight
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        if (modelData.account_id && modelData.mail_id) {
                            Qt.openUrlExternally(
                                "picacho://widget/message?account="
                                + encodeURIComponent(modelData.account_id)
                                + "&id=" + encodeURIComponent(modelData.mail_id)
                            );
                        } else {
                            Qt.openUrlExternally(root.packagesUri());
                        }
                    }

                    Rectangle {
                        anchors.fill: parent
                        radius: 6
                        color: pkgArea.containsMouse ? Kirigami.Theme.hoverColor : "transparent"
                        opacity: pkgArea.containsMouse ? 0.16 : 0
                    }

                    ColumnLayout {
                        id: pkgContent
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.margins: 6
                        spacing: 3

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                text: modelData.title || word("package", "Package")
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            PlasmaComponents.Label {
                                visible: !!(modelData.number && modelData.number.length > 0)
                                text: modelData.number || ""
                                font.pixelSize: 11
                                opacity: 0.6
                                elide: Text.ElideRight
                            }
                        }

                        PlasmaComponents.Label {
                            Layout.fillWidth: true
                            text: root.detailText(modelData)
                            font.pixelSize: 11
                            opacity: 0.72
                            elide: Text.ElideRight
                        }

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 3
                            Repeater {
                                model: 5
                                Rectangle {
                                    Layout.fillWidth: true
                                    height: 3
                                    radius: 1.5
                                    color: index <= (pkgArea.pkg.stage || 0)
                                        ? (pkgArea.pkg.delivered ? Kirigami.Theme.disabledTextColor : root.accentColor)
                                        : Kirigami.Theme.disabledTextColor
                                    opacity: index <= (pkgArea.pkg.stage || 0) ? 1.0 : 0.25
                                }
                            }
                        }
                    }
                }

                PlasmaComponents.Label {
                    anchors.centerIn: parent
                    visible: root.packages.length === 0
                    opacity: 0.65
                    text: word("no-recent-packages", "No recent packages")
                }
            }
        }
    }
}
