import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// Site Grabber, drawn from its view-model (Rust).
QQC2.ApplicationWindow {
    id: win

    // a list of links to show instead of scanning a page
    property var links: null
    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    signal requested(string what, var payload)

    title: "Site Grabber"
    width: 920
    height: 680
    minimumWidth: 680
    minimumHeight: 500

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    Component.onCompleted: vm.init("grabber", JSON.stringify(links || {}))
    // another list arrived while the window was open
    onLinksChanged: if (links && vm.state.length > 0) send("list", links)

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
        onAccepted: win.send("dir", decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }

    footer: QQC2.ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            QQC2.Button {
                text: "Add to Queue"
                enabled: win.s !== null && win.s.ready
                onClicked: win.send("add", false)
            }
            Item { Layout.fillWidth: true }
            QQC2.Button {
                text: "Cancel"
                onClicked: win.send("cancel")
            }
            QQC2.Button {
                text: win.s ? win.s.downloadLabel : "Download"
                icon.name: "download"
                highlighted: true
                enabled: win.s !== null && win.s.ready
                onClicked: win.send("add", true)
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.largeSpacing

        RowLayout {
            Layout.fillWidth: true
            visible: win.s !== null && !win.s.fromList
            StateField {
                Layout.fillWidth: true
                placeholderText: "https://example.com/gallery/"
                font.family: "monospace"
                value: win.s ? win.s.page : ""
                onEdited: text => win.send("page", text)
                onAccepted: win.send("find")
            }
            StateChoice {
                choices: [{ "id": "0", "name": "This page" }, { "id": "1", "name": "+1 level" }, { "id": "2", "name": "+2 levels" }]
                value: win.s ? String(win.s.depth) : "0"
                onChosen: id => win.send("depth", Number(id))
            }
            StateChoice {
                choices: win.s ? win.s.presets : []
                value: win.s ? win.s.preset : ""
                onChosen: id => win.send("preset", id)
            }
            QQC2.Button {
                text: win.s ? win.s.findLabel : "Find Files"
                highlighted: true
                enabled: win.s !== null && win.s.canFind
                onClicked: win.send("find")
            }
        }
        RowLayout {
            visible: win.s !== null && !win.s.fromList && (win.s.preset === "custom" || win.s.depth > 0)
            StateField {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 14
                visible: win.s !== null && win.s.preset === "custom"
                placeholderText: "jpg png zip"
                value: win.s ? win.s.custom : ""
                onEdited: text => win.send("custom", text)
            }
            QQC2.CheckBox {
                visible: win.s !== null && win.s.depth > 0
                text: "Stay on the same site"
                checked: win.s !== null && win.s.sameHost
                onToggled: win.send("same-host", checked)
            }
        }
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: win.s !== null && win.s.error.length > 0
            text: win.s ? win.s.error : ""
        }

        RowLayout {
            Layout.fillWidth: true
            QQC2.Label {
                textFormat: Text.PlainText
                text: win.s ? win.s.count : ""
                opacity: 0.7
            }
            QQC2.ToolButton {
                text: "Select All"
                onClicked: win.send("all", true)
            }
            QQC2.ToolButton {
                text: "None"
                onClicked: win.send("all", false)
            }
            Item { Layout.fillWidth: true }
            Kirigami.SearchField {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 12
                placeholderText: "Filter"
                onTextChanged: win.send("filter", text)
            }
        }

        QQC2.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            QQC2.ScrollBar.horizontal.policy: QQC2.ScrollBar.AlwaysOff

            ListView {
                id: list
                model: win.s ? win.s.links : []
                clip: true
                reuseItems: true

                Kirigami.PlaceholderMessage {
                    anchors.centerIn: parent
                    width: parent.width - Kirigami.Units.gridUnit * 4
                    visible: list.count === 0
                    icon.name: "folder-download-symbolic"
                    text: win.s ? win.s.empty : ""
                }

                delegate: QQC2.CheckDelegate {
                    id: link
                    required property var modelData
                    width: ListView.view.width
                    checked: modelData.on
                    onToggled: win.send("toggle", { "index": modelData.index, "value": checked })
                    contentItem: RowLayout {
                        spacing: Kirigami.Units.largeSpacing
                        QQC2.Label {
                            textFormat: Text.PlainText
                            Layout.preferredWidth: Kirigami.Units.gridUnit * 16
                            text: link.modelData.name
                            elide: Text.ElideMiddle
                        }
                        QQC2.Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            text: link.modelData.url
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                            elide: Text.ElideMiddle
                        }
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            QQC2.Label {
                textFormat: Text.PlainText
                text: "Save to"
                opacity: 0.7
            }
            QQC2.Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: win.s ? win.s.dirLabel : ""
                elide: Text.ElideMiddle
            }
            QQC2.Button {
                text: "Choose…"
                icon.name: "document-open-folder"
                onClicked: folderDialog.open()
            }
            QQC2.Button {
                visible: win.s !== null && win.s.dir.length > 0
                text: "Use Category Folders"
                onClicked: win.send("dir", "")
            }
            QQC2.Label {
                textFormat: Text.PlainText
                text: "Queue"
                opacity: 0.7
            }
            StateChoice {
                choices: win.s ? win.s.queues : []
                value: win.s ? win.s.queue : ""
                onChosen: id => win.send("queue", id)
            }
        }
    }
}
