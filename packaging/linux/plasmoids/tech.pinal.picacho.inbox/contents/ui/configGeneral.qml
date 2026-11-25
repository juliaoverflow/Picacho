import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import QtCore
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    id: configPage
    property alias cfg_accountId: accountIdHolder.text
    property alias cfg_unreadOnly: unreadOnlyCheck.checked

    property var accountModel: [{ id: "", label: word("all-inboxes", "All Inboxes") }]
    property var execSource: null

    Text {
        id: accountIdHolder
        visible: false
    }

    // The page's words in the language Picacho shows (the snapshot's
    // labels), else in English. %1 and %2 stand for a and b.
    property var labels: ({})
    function word(name, english, a, b) {
        var text = labels[name] || english;
        return text.replace("%1", function () { return a; }).replace("%2", function () { return b; });
    }

    function applyAccounts(snap) {
        labels = (snap && snap.labels) || {};
        var items = [{ id: "", label: word("all-inboxes", "All Inboxes") }];
        if (snap && snap.accounts) {
            for (var i = 0; i < snap.accounts.length; i++) {
                var a = snap.accounts[i];
                var label = (!a.name || a.name === a.email)
                    ? a.email
                    : word("account-name", "%1 (%2)", a.name, a.email);
                items.push({ id: a.id, label: label });
            }
        }
        accountModel = items;
        for (var j = 0; j < items.length; j++) {
            if (items[j].id === cfg_accountId) {
                accountCombo.currentIndex = j;
                break;
            }
        }
    }

    // Qt doesn't let QML's XMLHttpRequest read local files (unless
    // QML_XHR_ALLOW_FILE_READ is set), so the snapshot Picacho writes is read
    // through Plasma's executable engine instead, as main.qml does.
    function snapshotPath() {
        var base = StandardPaths.writableLocation(StandardPaths.GenericDataLocation).toString();
        if (base.startsWith("file://")) base = decodeURIComponent(base.substring(7));
        return base + "/picacho/widget-snapshot.json";
    }

    function shellQuote(s) {
        return "'" + s.replace(/'/g, "'\\''") + "'";
    }

    function loadAccounts() {
        try {
            if (!execSource) {
                execSource = Qt.createQmlObject(
                    'import org.kde.plasma.plasma5support as P5; P5.DataSource { engine: "executable" }',
                    configPage,
                    "picachoInboxConfigExec"
                );
                execSource.newData.connect(function(sourceName, data) {
                    execSource.disconnectSource(sourceName);
                    if (data && data.stdout) {
                        try { applyAccounts(JSON.parse(data.stdout)); } catch (e) {}
                    }
                });
            }
            execSource.connectSource(
                "cat " + shellQuote(snapshotPath()) + " 2>/dev/null || picacho --widget-json 2>/dev/null"
            );
        } catch (e) {}
    }

    Component.onCompleted: loadAccounts()

    Kirigami.FormLayout {
        QQC2.ComboBox {
            id: accountCombo
            Kirigami.FormData.label: word("account", "Account:")
            model: accountModel
            textRole: "label"
            onActivated: function(index) {
                if (index >= 0 && index < accountModel.length) {
                    cfg_accountId = accountModel[index].id;
                }
            }
        }

        QQC2.CheckBox {
            id: unreadOnlyCheck
            text: word("show-unread-only", "Show unread messages only")
        }
    }
}
