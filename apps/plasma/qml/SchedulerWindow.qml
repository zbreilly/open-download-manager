import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// Queues & Scheduler, drawn from its view-model (Rust).
QQC2.ApplicationWindow {
    id: win

    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    readonly property var e: s ? s.editor : null
    readonly property var dayNames: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
    signal requested(string what, var payload)

    title: "Queues & Scheduler"
    width: 880
    height: 700
    minimumWidth: 720
    minimumHeight: 540

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    Component.onCompleted: vm.init("scheduler", "{}")

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

    Kirigami.PromptDialog {
        id: newDialog
        title: "New Queue"
        standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
        onAccepted: {
            win.send("new", newName.text)
            newName.text = ""
        }
        QQC2.TextField {
            id: newName
            Layout.fillWidth: true
            placeholderText: "Name"
            onAccepted: newDialog.accept()
        }
    }

    footer: QQC2.ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            QQC2.Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: win.s === null ? "" : win.s.error.length > 0 ? win.s.error : win.e ? win.e.summary : ""
                color: win.s !== null && win.s.error.length > 0 ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                font: Kirigami.Theme.smallFont
                elide: Text.ElideRight
            }
            QQC2.Button {
                text: "Revert"
                enabled: win.s !== null && win.s.canSave
                onClicked: win.send("revert")
            }
            QQC2.Button {
                text: "Save Schedule"
                icon.name: "document-save"
                highlighted: true
                enabled: win.s !== null && win.s.canSave
                onClicked: win.send("save")
            }
        }
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // ------------------------------------------------------- queues
        ColumnLayout {
            Layout.fillHeight: true
            Layout.preferredWidth: Kirigami.Units.gridUnit * 13
            Layout.maximumWidth: Kirigami.Units.gridUnit * 13
            spacing: 0

            QQC2.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                ListView {
                    model: win.s ? win.s.queues : []
                    clip: true
                    delegate: SidebarItem {
                        required property var modelData
                        width: ListView.view.width
                        text: modelData.name
                        iconName: modelData.icon
                        count: modelData.count
                        checked: win.s !== null && win.s.selected === modelData.id
                        onClicked: win.send("pick", modelData.id)
                    }
                }
            }
            QQC2.Button {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.smallSpacing
                text: "New Queue"
                icon.name: "list-add"
                onClicked: newDialog.open()
            }
        }
        Kirigami.Separator {
            Layout.fillHeight: true
        }

        // ------------------------------------------------------- editor
        Kirigami.PlaceholderMessage {
            Layout.fillWidth: true
            visible: win.e === null
            text: "Select a queue"
        }
        QQC2.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: win.e !== null
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width
                spacing: Kirigami.Units.largeSpacing

                RowLayout {
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    Kirigami.Heading {
                        textFormat: Text.PlainText
                        level: 2
                        text: win.e ? win.e.name : ""
                    }
                    QQC2.Label {
                        textFormat: Text.PlainText
                        visible: win.e !== null && win.e.running
                        text: "● Running"
                        color: Kirigami.Theme.positiveTextColor
                    }
                    Item { Layout.fillWidth: true }
                    QQC2.Button {
                        text: "Start Now"
                        icon.name: "media-playback-start"
                        enabled: win.e !== null && !win.e.running
                        onClicked: win.send("start")
                    }
                    QQC2.Button {
                        text: "Stop"
                        icon.name: "media-playback-stop"
                        enabled: win.e !== null && win.e.running
                        onClicked: win.send("stop")
                    }
                }

                Kirigami.FormLayout {
                    Layout.fillWidth: true

                    StateField {
                        Kirigami.FormData.label: "Name:"
                        Layout.fillWidth: true
                        visible: win.e !== null && !win.e.builtin
                        value: win.e ? win.e.name : ""
                        onEdited: text => win.send("name", text)
                    }

                    Kirigami.Separator {
                        Kirigami.FormData.isSection: true
                        Kirigami.FormData.label: "Schedule"
                    }
                    RowLayout {
                        Kirigami.FormData.label: "Start at:"
                        QQC2.SpinBox {
                            from: 0
                            to: 23
                            enabled: win.e !== null && win.e.startEnabled
                            value: win.e ? win.e.startHour : 0
                            onValueModified: win.send("time", { "field": "startTime", "hour": value, "minute": win.e.startMinute })
                        }
                        QQC2.Label { textFormat: Text.PlainText; text: ":" }
                        QQC2.SpinBox {
                            from: 0
                            to: 59
                            enabled: win.e !== null && win.e.startEnabled
                            value: win.e ? win.e.startMinute : 0
                            onValueModified: win.send("time", { "field": "startTime", "hour": win.e.startHour, "minute": value })
                        }
                        QQC2.Switch {
                            checked: win.e !== null && win.e.startEnabled
                            onToggled: win.send("flag", { "field": "startEnabled", "value": checked })
                        }
                    }
                    RowLayout {
                        Kirigami.FormData.label: "Stop at:"
                        QQC2.SpinBox {
                            from: 0
                            to: 23
                            enabled: win.e !== null && win.e.stopEnabled
                            value: win.e ? win.e.stopHour : 0
                            onValueModified: win.send("time", { "field": "stopTime", "hour": value, "minute": win.e.stopMinute })
                        }
                        QQC2.Label { textFormat: Text.PlainText; text: ":" }
                        QQC2.SpinBox {
                            from: 0
                            to: 59
                            enabled: win.e !== null && win.e.stopEnabled
                            value: win.e ? win.e.stopMinute : 0
                            onValueModified: win.send("time", { "field": "stopTime", "hour": win.e.stopHour, "minute": value })
                        }
                        QQC2.Switch {
                            checked: win.e !== null && win.e.stopEnabled
                            onToggled: win.send("flag", { "field": "stopEnabled", "value": checked })
                        }
                    }
                    RowLayout {
                        Kirigami.FormData.label: "Repeat on:"
                        spacing: Kirigami.Units.smallSpacing
                        Repeater {
                            model: win.dayNames
                            delegate: QQC2.Button {
                                required property string modelData
                                required property int index
                                text: modelData
                                checkable: true
                                checked: win.e !== null && win.e.days[index]
                                onToggled: win.send("day", { "index": index, "value": checked })
                            }
                        }
                    }
                    QQC2.Label {
                        textFormat: Text.PlainText
                        Kirigami.FormData.label: "Next run:"
                        visible: win.e !== null && win.e.nextRun !== null
                        text: visible ? win.e.nextRun : ""
                    }

                    Kirigami.Separator {
                        Kirigami.FormData.isSection: true
                        Kirigami.FormData.label: "Behavior"
                    }
                    QQC2.SpinBox {
                        Kirigami.FormData.label: "Files at the same time:"
                        from: 1
                        to: 32
                        value: win.e ? win.e.maxConcurrent : 1
                        onValueModified: win.send("concurrent", value)
                    }
                    StateChoice {
                        Kirigami.FormData.label: "Retry failed downloads:"
                        choices: [
                            { "id": "0", "name": "Never" }, { "id": "1", "name": "Once" }, { "id": "3", "name": "3 times" },
                            { "id": "5", "name": "5 times" }, { "id": "10", "name": "10 times" }
                        ]
                        value: win.e ? win.e.retryFailed : "0"
                        onChosen: id => win.send("retry", id)
                    }
                    StateChoice {
                        Kirigami.FormData.label: "When the queue finishes:"
                        choices: [
                            { "id": "none", "name": "Do nothing" }, { "id": "sleep", "name": "Put the computer to sleep" },
                            { "id": "shutdown", "name": "Shut down the computer" }, { "id": "exit", "name": "Quit Open Download Manager" }
                        ]
                        value: win.e ? win.e.onFinish : "none"
                        onChosen: id => win.send("on-finish", id)
                    }
                    QQC2.Label {
                        textFormat: Text.PlainText
                        visible: win.e !== null && win.e.countdownNote
                        text: "A 60-second countdown with a Cancel button appears first."
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                    }

                    Kirigami.Separator {
                        Kirigami.FormData.isSection: true
                        Kirigami.FormData.label: "Order"
                    }
                }

                QQC2.Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: win.e !== null && win.e.order.length === 0
                    text: "No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding."
                    wrapMode: Text.Wrap
                    opacity: 0.7
                }
                Repeater {
                    model: win.e ? win.e.order : []
                    delegate: RowLayout {
                        id: item
                        required property var modelData
                        required property int index
                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.largeSpacing
                        Layout.rightMargin: Kirigami.Units.largeSpacing
                        QQC2.Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            text: item.modelData.name
                            elide: Text.ElideMiddle
                        }
                        QQC2.Label {
                            textFormat: Text.PlainText
                            text: item.modelData.info
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                        }
                        QQC2.ToolButton {
                            icon.name: "go-up"
                            text: "Move up"
                            display: QQC2.AbstractButton.IconOnly
                            enabled: item.index > 0
                            onClicked: win.send("move", { "from": item.index, "to": item.index - 1 })
                        }
                        QQC2.ToolButton {
                            icon.name: "go-down"
                            text: "Move down"
                            display: QQC2.AbstractButton.IconOnly
                            enabled: win.e !== null && item.index < win.e.order.length - 1
                            onClicked: win.send("move", { "from": item.index, "to": item.index + 1 })
                        }
                    }
                }

                QQC2.Button {
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: win.e !== null && !win.e.builtin
                    text: "Delete Queue"
                    icon.name: "edit-delete"
                    onClicked: win.send("delete")
                }
            }
        }
    }
}
