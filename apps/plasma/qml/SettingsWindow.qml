import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// Settings, drawn from its view-model (Rust). Changes apply as you make
// them.
QQC2.ApplicationWindow {
    id: win

    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    readonly property var v: s ? s.v : ({})
    // what the folder dialog is choosing for: a setting key, or "category:<id>"
    property string choosing: ""
    signal requested(string what, var payload)

    title: "Settings"
    width: 860
    height: 680
    minimumWidth: 640
    minimumHeight: 480

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    function set(key, value) {
        send("set", { "key": key, "value": value })
    }

    Component.onCompleted: vm.init("settings", "{}")
    // pending edits are saved right away
    onClosing: send("flush")

    ViewModel {
        id: vm
        onRequest: (what, payload) => {
            if (what === "close")
                win.close()
            else
                win.requested(what, payload.length > 0 ? JSON.parse(payload) : null)
        }
    }

    Connections {
        target: Backend
        function onChanged(kind) {
            vm.storeChanged(kind)
        }
    }

    Shortcut {
        sequence: StandardKey.Cancel
        onActivated: win.close()
    }

    FolderDialog {
        id: folderDialog
        title: "Choose a Folder"
        onAccepted: {
            const path = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
            if (win.choosing.startsWith("category:"))
                win.send("category-folder", { "id": win.choosing.substring(9), "value": path })
            else
                win.set(win.choosing, path)
        }
    }

    // one switch bound to a boolean setting
    component Toggle: QQC2.CheckBox {
        required property string key
        checked: win.v[key] === true
        onToggled: win.set(key, checked)
    }

    // one number bound to a numeric setting
    component Number: QQC2.SpinBox {
        required property string key
        editable: true
        value: win.v[key] !== undefined ? win.v[key] : 0
        onValueModified: win.set(key, value)
    }

    header: QQC2.TabBar {
        id: tabs
        QQC2.TabButton { text: "General" }
        QQC2.TabButton { text: "Connection" }
        QQC2.TabButton { text: "Browser" }
        QQC2.TabButton { text: "File Types" }
        QQC2.TabButton { text: "Proxy" }
        QQC2.TabButton { text: "Advanced" }
    }

    StackLayout {
        anchors.fill: parent
        currentIndex: tabs.currentIndex

        // ---------------------------------------------------- General
        QQC2.ScrollView {
            contentWidth: availableWidth
            Kirigami.FormLayout {
                width: parent.width
                Toggle { key: "launchAtLogin"; text: "Open at login" }
                Toggle { key: "minimizeToTray"; text: "Keep running in the system tray when the window is closed" }
                Toggle { key: "clipboardMonitor"; text: "Watch the clipboard for download links" }
                QQC2.Label {
                    textFormat: Text.PlainText
                    text: "Copying a link to a file type listed under Browser opens the Add window."
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                }
                Toggle { key: "showProgressOnStart"; text: "Show the progress window when a download starts" }
                Toggle { key: "showCompleteDialog"; text: "Show the download window when a download completes" }
                Toggle { key: "confirmDelete"; text: "Confirm before deleting downloads" }
                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Notifications"
                }
                Toggle { key: "notifyComplete"; text: "Download complete" }
                Toggle { key: "notifyFailure"; text: "Download failed" }
                Toggle { key: "notifyQueueFinished"; text: "Queue finished" }
            }
        }

        // ------------------------------------------------- Connection
        QQC2.ScrollView {
            contentWidth: availableWidth
            Kirigami.FormLayout {
                width: parent.width
                Number {
                    Kirigami.FormData.label: "Connections per download:"
                    key: "defaultConnections"
                    from: 1
                    to: 32
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                    text: "Files are split dynamically: a connection that finishes takes over half of the largest remaining piece."
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    wrapMode: Text.Wrap
                }
                Number {
                    Kirigami.FormData.label: "Simultaneous downloads:"
                    key: "maxSimultaneous"
                    from: 1
                    to: 64
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    text: "Outside of queues. Each queue sets its own limit."
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                }
                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Speed limits"
                }
                RowLayout {
                    Kirigami.FormData.label: "Global limit (all downloads combined):"
                    StateField {
                        Layout.preferredWidth: Kirigami.Units.gridUnit * 5
                        value: win.s ? win.s.globalLimit.text : ""
                        onEdited: text => win.send("speed", { "key": "globalLimitBps", "text": text })
                    }
                    QQC2.Label { textFormat: Text.PlainText; text: "MB/s"; opacity: 0.7 }
                    QQC2.Switch {
                        checked: win.v.globalLimitEnabled === true
                        onToggled: win.set("globalLimitEnabled", checked)
                    }
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    visible: text.length > 0
                    text: win.s ? win.s.globalLimit.error : ""
                    color: Kirigami.Theme.negativeTextColor
                    font: Kirigami.Theme.smallFont
                }
                RowLayout {
                    Kirigami.FormData.label: "Per-download limit:"
                    StateField {
                        Layout.preferredWidth: Kirigami.Units.gridUnit * 5
                        value: win.s ? win.s.perDownloadLimit.text : ""
                        onEdited: text => win.send("speed", { "key": "perDownloadLimitBps", "text": text })
                    }
                    QQC2.Label { textFormat: Text.PlainText; text: "MB/s"; opacity: 0.7 }
                    QQC2.Switch {
                        checked: win.v.perDownloadLimitEnabled === true
                        onToggled: win.set("perDownloadLimitEnabled", checked)
                    }
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    visible: text.length > 0
                    text: win.s ? win.s.perDownloadLimit.error : ""
                    color: Kirigami.Theme.negativeTextColor
                    font: Kirigami.Theme.smallFont
                }
                Toggle { key: "limitOnStartup"; text: "Turn on the global limit when the app starts" }
            }
        }

        // ---------------------------------------------------- Browser
        QQC2.ScrollView {
            contentWidth: availableWidth
            Kirigami.FormLayout {
                width: parent.width
                RowLayout {
                    Kirigami.FormData.label: "Extension:"
                    QQC2.Label {
                        textFormat: Text.PlainText
                        text: win.s ? (win.s.extension.state === "idle" ? "○ " : "● ") + win.s.extension.text : ""
                        color: win.s === null ? Kirigami.Theme.textColor
                             : win.s.extension.state === "error" ? Kirigami.Theme.negativeTextColor
                             : win.s.extension.state === "complete" ? Kirigami.Theme.positiveTextColor
                             : Kirigami.Theme.disabledTextColor
                    }
                    QQC2.Button {
                        text: "Show Extension Folder"
                        icon.name: "document-open-folder"
                        onClicked: win.send("show-extension")
                    }
                }
                RowLayout {
                    Kirigami.FormData.label: "Pairing code:"
                    QQC2.Button {
                        text: "Copy"
                        icon.name: "edit-copy"
                        onClicked: win.send("copy-pairing")
                    }
                    QQC2.Button {
                        text: "New Code"
                        icon.name: "view-refresh"
                        onClicked: win.send("new-pairing")
                    }
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 28
                    text: "Paste it into the extension’s popup, under Connection. Only a paired extension can talk to the app, and other programs can’t read what it sends."
                    font: Kirigami.Theme.smallFont
                    color: Kirigami.Theme.disabledTextColor
                    wrapMode: Text.Wrap
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 28
                    visible: win.s !== null && win.s.extension.error !== null
                    text: visible ? win.s.extension.error : ""
                    color: Kirigami.Theme.negativeTextColor
                    font: Kirigami.Theme.smallFont
                    wrapMode: Text.Wrap
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 28
                    text: "Chrome, Edge, Brave, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json."
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    wrapMode: Text.Wrap
                }
                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Capture"
                }
                Toggle { key: "captureEnabled"; text: "Take over browser downloads" }
                QQC2.CheckBox {
                    text: "Pause capture for 10 minutes"
                    checked: win.s !== null && win.s.capturePaused
                    onToggled: win.send("pause-capture", checked)
                }
                StateChoice {
                    Kirigami.FormData.label: "Bypass key (hold while clicking):"
                    choices: [{ "id": "alt", "name": "Alt" }, { "id": "shift", "name": "Shift" }, { "id": "ctrl", "name": "Ctrl" }, { "id": "none", "name": "None" }]
                    value: win.v.bypassKey || ""
                    onChosen: id => win.set("bypassKey", id)
                }
                Number {
                    Kirigami.FormData.label: "Skip files smaller than (KB):"
                    key: "captureMinSizeKb"
                    from: 0
                    to: 10000000
                }
                StateField {
                    Kirigami.FormData.label: "File types to capture:"
                    Layout.fillWidth: true
                    Layout.minimumWidth: Kirigami.Units.gridUnit * 24
                    font.family: "monospace"
                    value: win.s ? win.s.captureTypes : ""
                    onEditingFinished: win.send("capture-types", text)
                }
            }
        }

        // ------------------------------------------------- File Types
        QQC2.ScrollView {
            contentWidth: availableWidth
            ColumnLayout {
                width: parent.width
                spacing: Kirigami.Units.largeSpacing

                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    text: "Each category has its own file types and folder."
                    opacity: 0.7
                }
                Repeater {
                    model: win.s ? win.s.categories : []
                    delegate: Kirigami.AbstractCard {
                        id: card
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.largeSpacing
                        Layout.rightMargin: Kirigami.Units.largeSpacing

                        contentItem: Kirigami.FormLayout {
                            Kirigami.Heading {
                                textFormat: Text.PlainText
                                level: 4
                                text: card.modelData.name + " — " + card.modelData.summary
                            }
                            StateField {
                                Kirigami.FormData.label: "Name:"
                                Layout.fillWidth: true
                                visible: !card.modelData.general
                                value: card.modelData.name
                                onEditingFinished: win.send("category-name", { "id": card.modelData.id, "value": text })
                            }
                            QQC2.Label {
                                textFormat: Text.PlainText
                                Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                                visible: card.modelData.general
                                text: "General holds every file type no other category claims, and saves to your download folder."
                                font: Kirigami.Theme.smallFont
                                opacity: 0.7
                                wrapMode: Text.Wrap
                            }
                            RowLayout {
                                Kirigami.FormData.label: "Folder:"
                                Layout.fillWidth: true
                                QQC2.Label {
                                    textFormat: Text.PlainText
                                    Layout.fillWidth: true
                                    text: card.modelData.folder
                                    opacity: card.modelData.ownFolder ? 1 : 0.7
                                    elide: Text.ElideMiddle
                                }
                                QQC2.Button {
                                    text: "Choose…"
                                    onClicked: {
                                        win.choosing = "category:" + card.modelData.id
                                        folderDialog.open()
                                    }
                                }
                                QQC2.Button {
                                    visible: card.modelData.ownFolder
                                    text: "Use Default"
                                    onClicked: win.send("category-folder", { "id": card.modelData.id, "value": "" })
                                }
                            }
                            StateField {
                                Kirigami.FormData.label: "File types:"
                                Layout.fillWidth: true
                                visible: !card.modelData.general
                                font.family: "monospace"
                                placeholderText: "mkv webm"
                                value: card.modelData.extensions
                                onEditingFinished: win.send("category-types", { "id": card.modelData.id, "value": text })
                            }
                            QQC2.Button {
                                visible: !card.modelData.general
                                text: "Remove This Category"
                                icon.name: "edit-delete"
                                onClicked: win.send("remove-category", { "id": card.modelData.id })
                            }
                        }
                    }
                }
                QQC2.Button {
                    Layout.margins: Kirigami.Units.largeSpacing
                    text: "Add Category"
                    icon.name: "list-add"
                    onClicked: win.send("add-category")
                }
            }
        }

        // ------------------------------------------------------ Proxy
        QQC2.ScrollView {
            contentWidth: availableWidth
            Kirigami.FormLayout {
                width: parent.width
                StateChoice {
                    Kirigami.FormData.label: "Connection:"
                    choices: [
                        { "id": "none", "name": "No proxy" }, { "id": "system", "name": "Use system settings" },
                        { "id": "http", "name": "HTTP proxy" }, { "id": "socks5", "name": "SOCKS5 proxy" }
                    ]
                    value: win.v.proxyMode || ""
                    onChosen: id => win.set("proxyMode", id)
                }
                StateField {
                    Kirigami.FormData.label: "Host:"
                    visible: win.s !== null && win.s.manualProxy
                    value: win.v.proxyHost || ""
                    onEdited: text => win.set("proxyHost", text)
                }
                Number {
                    Kirigami.FormData.label: "Port:"
                    visible: win.s !== null && win.s.manualProxy
                    key: "proxyPort"
                    from: 1
                    to: 65535
                }
                StateField {
                    Kirigami.FormData.label: "Username:"
                    visible: win.s !== null && win.s.manualProxy
                    value: win.v.proxyUser || ""
                    onEdited: text => win.set("proxyUser", text)
                }
                Kirigami.PasswordField {
                    Kirigami.FormData.label: "Password:"
                    visible: win.s !== null && win.s.manualProxy
                    onTextEdited: win.set("proxyPass", text)
                    Component.onCompleted: text = win.v.proxyPass || ""
                }
            }
        }

        // --------------------------------------------------- Advanced
        QQC2.ScrollView {
            contentWidth: availableWidth
            Kirigami.FormLayout {
                width: parent.width
                RowLayout {
                    Kirigami.FormData.label: "Download folder:"
                    QQC2.Label {
                        textFormat: Text.PlainText
                        Layout.maximumWidth: Kirigami.Units.gridUnit * 20
                        text: win.v.defaultFolder || ""
                        elide: Text.ElideMiddle
                    }
                    QQC2.Button {
                        text: "Choose…"
                        icon.name: "document-open-folder"
                        onClicked: {
                            win.choosing = "defaultFolder"
                            folderDialog.open()
                        }
                    }
                }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                    text: "Each category saves to a subfolder of this folder unless you choose another under File Types."
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    wrapMode: Text.Wrap
                }
                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Network"
                }
                StateField {
                    Kirigami.FormData.label: "User agent:"
                    Layout.minimumWidth: Kirigami.Units.gridUnit * 24
                    value: win.v.userAgent || ""
                    onEdited: text => win.set("userAgent", text)
                }
                Number { Kirigami.FormData.label: "Retries per connection:"; key: "maxRetries"; from: 0; to: 1000 }
                Number { Kirigami.FormData.label: "Retry delay (seconds):"; key: "retryDelaySecs"; from: 0; to: 3600 }
                Number { Kirigami.FormData.label: "Timeout (seconds):"; key: "timeoutSecs"; from: 1; to: 3600 }
                Number { Kirigami.FormData.label: "Minimum segment size (KB):"; key: "minSegmentKb"; from: 1; to: 1048576 }
                Number { Kirigami.FormData.label: "Extension port:"; key: "extensionPort"; from: 1; to: 65535 }
                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Security"
                }
                Toggle { key: "verifyTls"; text: "Verify TLS certificates" }
                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                    visible: win.s !== null && win.s.tlsWarning
                    text: "Certificate checks are off. Anyone on the network path can alter what you download."
                    color: Kirigami.Theme.negativeTextColor
                    wrapMode: Text.Wrap
                }
            }
        }
    }
}
