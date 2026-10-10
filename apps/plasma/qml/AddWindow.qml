import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// The Add Download window: draws the form its view-model (Rust) describes.
QQC2.ApplicationWindow {
    id: win

    // the request this window was opened with
    property var params: ({})
    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    signal requested(string what, var payload)

    title: "Add Download"
    width: 580
    height: 760
    minimumWidth: 480
    minimumHeight: 520

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    function text(field, value) {
        send("text", { "field": field, "value": value })
    }

    Component.onCompleted: vm.init("add", JSON.stringify(params))

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
    // Enter starts the download, except for a link a web page chose
    Shortcut {
        sequences: ["Return", "Enter"]
        enabled: win.s !== null && !win.s.fromWeb && !win.s.batch
        onActivated: win.send("submit", true)
    }

    FolderDialog {
        id: folderDialog
        title: "Choose a Folder"
        onAccepted: win.text("dir", decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }

    // the file exists already
    Kirigami.PromptDialog {
        visible: win.s !== null && win.s.exists !== null
        title: visible ? "“" + win.s.exists + "” already exists in this folder." : ""
        subtitle: "Rename the new file, resume the existing one, or replace it."
        standardButtons: Kirigami.Dialog.Cancel
        customFooterActions: [
            Kirigami.Action { text: "Overwrite"; icon.name: "edit-delete"; onTriggered: win.send("exists", "overwrite") },
            Kirigami.Action { text: "Resume Existing File"; onTriggered: win.send("exists", "resume") },
            Kirigami.Action { text: "Rename New File"; onTriggered: win.send("exists", "rename") }
        ]
        onRejected: win.send("exists", "cancel")
    }

    footer: QQC2.ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            QQC2.Switch {
                text: "Batch"
                checked: win.s !== null && win.s.batch
                onToggled: win.send("flag", { "flag": "batch", "value": checked })
                QQC2.ToolTip.text: "Add several links or a numbered pattern"
                QQC2.ToolTip.visible: hovered
            }
            Item { Layout.fillWidth: true }
            QQC2.Button {
                text: "Add to Queue"
                enabled: win.s !== null && win.s.ready
                onClicked: win.send("submit", false)
            }
            QQC2.Button {
                text: "Cancel"
                onClicked: win.send("cancel")
            }
            QQC2.Button {
                text: win.s ? win.s.startLabel : "Start Download"
                icon.name: "download"
                highlighted: true
                enabled: win.s !== null && win.s.ready
                onClicked: win.send("submit", true)
            }
        }
    }

    QQC2.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: Kirigami.Units.largeSpacing

            Kirigami.FormLayout {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                visible: win.s !== null

                // ----------------------------------------------- the link
                StateField {
                    Kirigami.FormData.label: "URL:"
                    Layout.fillWidth: true
                    visible: win.s !== null && !win.s.batch
                    placeholderText: "https://"
                    font.family: "monospace"
                    value: win.s ? win.s.url : ""
                    onEdited: text => win.text("url", text)
                }
                QQC2.Label {
                    visible: win.s !== null && win.s.urlError
                    text: "Enter a full http:// or https:// address."
                    color: Kirigami.Theme.negativeTextColor
                }
                RowLayout {
                    visible: win.s !== null && win.s.info !== null
                    Layout.fillWidth: true
                    QQC2.BusyIndicator {
                        visible: win.s !== null && win.s.info !== null && win.s.info.spinning
                        running: visible
                        implicitWidth: Kirigami.Units.iconSizes.smallMedium
                        implicitHeight: Kirigami.Units.iconSizes.smallMedium
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        QQC2.Label {
                            Layout.fillWidth: true
                            text: win.s !== null && win.s.info !== null ? win.s.info.title : ""
                            wrapMode: Text.Wrap
                        }
                        QQC2.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: win.s !== null && win.s.info !== null ? win.s.info.detail : ""
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                            wrapMode: Text.Wrap
                        }
                    }
                    QQC2.Button {
                        visible: win.s !== null && win.s.check.length > 0
                        text: win.s ? win.s.check : ""
                        onClicked: win.send("check")
                    }
                }
                QQC2.TextArea {
                    Kirigami.FormData.label: "Links:"
                    Layout.fillWidth: true
                    Layout.minimumHeight: Kirigami.Units.gridUnit * 6
                    visible: win.s !== null && win.s.batch
                    font.family: "monospace"
                    placeholderText: "One per line. Patterns like file[001-120].jpg or [a-z] expand."
                    wrapMode: TextEdit.WrapAnywhere
                    onTextChanged: if (activeFocus) win.text("batchText", text)
                }
                QQC2.Label {
                    Kirigami.FormData.label: "Files:"
                    visible: win.s !== null && win.s.batch
                    text: win.s ? win.s.batchCount : ""
                }

                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Save"
                }
                StateField {
                    Kirigami.FormData.label: "File name:"
                    Layout.fillWidth: true
                    visible: win.s !== null && !win.s.batch
                    value: win.s ? win.s.name : ""
                    onEdited: text => win.text("name", text)
                }
                StateChoice {
                    Kirigami.FormData.label: "Category:"
                    visible: win.s !== null && !win.s.batch
                    choices: win.s ? win.s.categories : []
                    value: win.s ? win.s.category : ""
                    onChosen: id => win.send("category", id)
                }
                StateChoice {
                    Kirigami.FormData.label: "Queue:"
                    choices: win.s ? win.s.queues : []
                    value: win.s ? win.s.queue : ""
                    onChosen: id => win.send("queue", id)
                }
                RowLayout {
                    Kirigami.FormData.label: "Save to:"
                    Layout.fillWidth: true
                    StateField {
                        Layout.fillWidth: true
                        font.family: "monospace"
                        value: win.s ? win.s.dir : ""
                        onEdited: text => win.text("dir", text)
                    }
                    QQC2.Button {
                        icon.name: "document-open-folder"
                        text: "Choose…"
                        onClicked: recentMenu.count > 1 ? recentMenu.popup() : folderDialog.open()
                        QQC2.Menu {
                            id: recentMenu
                            Instantiator {
                                model: win.s ? win.s.recents : []
                                delegate: QQC2.MenuItem {
                                    required property string modelData
                                    text: modelData
                                    onTriggered: win.text("dir", modelData)
                                }
                                onObjectAdded: (index, object) => recentMenu.insertItem(index, object)
                                onObjectRemoved: (index, object) => recentMenu.removeItem(object)
                            }
                            QQC2.MenuItem {
                                text: "Choose…"
                                onTriggered: folderDialog.open()
                            }
                        }
                    }
                }

                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Options"
                }
                StateChoice {
                    Kirigami.FormData.label: "Connections:"
                    enabled: win.s !== null && win.s.canSplit
                    choices: win.s ? win.s.connectionChoices.map(c => ({ "id": String(c), "name": String(c) })) : []
                    value: win.s ? String(win.s.connections) : ""
                    onChosen: id => win.send("connections", Number(id))
                }
                QQC2.CheckBox {
                    visible: win.s !== null && win.s.showsAlways
                    text: win.s ? "Always put ." + win.s.nameExt + " files in " + win.s.categoryName : ""
                    checked: win.s !== null && win.s.always
                    onToggled: win.send("flag", { "flag": "always", "value": checked })
                }
                QQC2.CheckBox {
                    visible: win.s !== null && !win.s.batch
                    text: win.s ? "Remember this folder for " + win.s.categoryName : ""
                    checked: win.s !== null && win.s.remember
                    onToggled: win.send("flag", { "flag": "remember", "value": checked })
                }
                QQC2.CheckBox {
                    visible: win.s !== null && !win.s.batch
                    text: "Verify checksum after download"
                    checked: win.s !== null && win.s.verify
                    onToggled: win.send("flag", { "flag": "verify", "value": checked })
                }
                StateField {
                    Kirigami.FormData.label: "Checksum:"
                    Layout.fillWidth: true
                    visible: win.s !== null && !win.s.batch && win.s.verify
                    placeholderText: "SHA-256, SHA-1 or MD5"
                    font.family: "monospace"
                    value: win.s ? win.s.checksum : ""
                    onEdited: text => win.text("checksum", text)
                }

                Kirigami.Separator {
                    Kirigami.FormData.isSection: true
                    Kirigami.FormData.label: "Authentication, referrer, cookies, headers"
                }
                StateField {
                    Kirigami.FormData.label: "Username:"
                    Layout.fillWidth: true
                    value: win.s ? win.s.username : ""
                    onEdited: text => win.text("username", text)
                }
                Kirigami.PasswordField {
                    Kirigami.FormData.label: "Password:"
                    Layout.fillWidth: true
                    onTextEdited: win.text("password", text)
                }
                StateField {
                    Kirigami.FormData.label: "Referrer:"
                    Layout.fillWidth: true
                    value: win.s ? win.s.referrer : ""
                    onEdited: text => win.text("referrer", text)
                }
                StateField {
                    Kirigami.FormData.label: "Cookies:"
                    Layout.fillWidth: true
                    placeholderText: "name=value; other=value"
                    value: win.s ? win.s.cookies : ""
                    onEdited: text => win.text("cookies", text)
                }
                StateField {
                    Kirigami.FormData.label: "User agent:"
                    Layout.fillWidth: true
                    placeholderText: "Default"
                    value: win.s ? win.s.userAgent : ""
                    onEdited: text => win.text("userAgent", text)
                }
                Repeater {
                    model: win.s ? win.s.headers : []
                    delegate: RowLayout {
                        id: header
                        required property var modelData
                        required property int index
                        Kirigami.FormData.label: index === 0 ? "Headers:" : ""
                        Layout.fillWidth: true
                        StateField {
                            Layout.fillWidth: true
                            placeholderText: "Header"
                            value: header.modelData[0]
                            onEdited: text => win.send("header", { "index": header.index, "part": "name", "value": text })
                        }
                        StateField {
                            Layout.fillWidth: true
                            placeholderText: "Value"
                            value: header.modelData[1]
                            onEdited: text => win.send("header", { "index": header.index, "part": "value", "value": text })
                        }
                        QQC2.ToolButton {
                            icon.name: "list-remove"
                            text: "Remove header"
                            display: QQC2.AbstractButton.IconOnly
                            onClicked: win.send("remove-header", header.index)
                        }
                    }
                }
                QQC2.Button {
                    text: "Add Header"
                    icon.name: "list-add"
                    onClicked: win.send("add-header")
                }
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                type: Kirigami.MessageType.Error
                visible: win.s !== null && win.s.error.length > 0
                text: win.s ? win.s.error : ""
            }
        }
    }
}
