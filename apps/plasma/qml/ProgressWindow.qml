import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// Per-download progress window, drawn from its view-model (Rust).
QQC2.ApplicationWindow {
    id: win

    property string downloadId: ""
    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    signal requested(string what, var payload)

    title: s ? s.title : "Download"
    width: 780
    height: 700
    minimumWidth: 620
    minimumHeight: 520

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    function statusColor(status) {
        switch (status) {
        case "downloading":
        case "connecting":
        case "assembling": return Kirigami.Theme.highlightColor
        case "paused": return Kirigami.Theme.neutralTextColor
        case "error": return Kirigami.Theme.negativeTextColor
        case "complete": return Kirigami.Theme.positiveTextColor
        }
        return Kirigami.Theme.disabledTextColor
    }

    Component.onCompleted: vm.init("progress", JSON.stringify(downloadId))

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

    // the pointer over the window stops it from closing by itself
    HoverHandler {
        onHoveredChanged: win.send("hover", hovered)
    }

    Kirigami.PromptDialog {
        id: cancelDialog
        title: "Cancel this download?"
        subtitle: "The partial file is deleted and the download is removed from the list."
        standardButtons: Kirigami.Dialog.NoButton
        customFooterActions: [
            Kirigami.Action { text: "Keep Downloading"; onTriggered: cancelDialog.close() },
            Kirigami.Action {
                text: "Cancel Download"
                icon.name: "edit-delete"
                onTriggered: {
                    cancelDialog.close()
                    win.send("cancel")
                }
            }
        ]
    }

    footer: QQC2.ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            QQC2.Button {
                text: win.s !== null && win.s.complete ? "Close" : "Hide"
                onClicked: win.send("hide")
            }
            QQC2.Label {
                text: win.s !== null && win.s.closing ? win.s.closing : ""
                font: Kirigami.Theme.smallFont
                opacity: 0.7
            }
            Item { Layout.fillWidth: true }
            QQC2.Button {
                visible: win.s !== null && win.s.exists && !win.s.complete
                text: "Cancel Download"
                onClicked: cancelDialog.open()
            }
            QQC2.Button {
                visible: win.s !== null && win.s.exists && !win.s.complete
                text: win.s !== null && win.s.exists ? win.s.toggleLabel : ""
                highlighted: true
                onClicked: win.send("toggle")
            }
            QQC2.Button {
                visible: win.s !== null && win.s.exists && win.s.complete
                text: "Show in Folder"
                icon.name: "document-open-folder"
                onClicked: win.send("reveal")
            }
            QQC2.Button {
                visible: win.s !== null && win.s.exists && win.s.complete
                text: "Open"
                icon.name: "document-open"
                highlighted: true
                onClicked: win.send("open")
            }
        }
    }

    Kirigami.PlaceholderMessage {
        anchors.centerIn: parent
        visible: win.s !== null && !win.s.exists
        text: "This download no longer exists."
    }

    QQC2.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        visible: win.s !== null && win.s.exists

        ColumnLayout {
            width: parent.width
            spacing: 0

            ColumnLayout {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.gridUnit
                spacing: Kirigami.Units.largeSpacing

                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.largeSpacing
                    Kirigami.Icon {
                        source: win.s !== null && win.s.exists ? win.s.icon : ""
                        implicitWidth: Kirigami.Units.iconSizes.large
                        implicitHeight: Kirigami.Units.iconSizes.large
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Kirigami.Heading {
                            Layout.fillWidth: true
                            level: 3
                            text: win.s !== null && win.s.exists ? win.s.name : ""
                            elide: Text.ElideMiddle
                        }
                        QQC2.Label {
                            Layout.fillWidth: true
                            text: win.s !== null && win.s.exists ? win.s.place : ""
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                            elide: Text.ElideMiddle
                        }
                    }
                    QQC2.Label {
                        text: win.s !== null && win.s.exists ? "● " + win.s.statusLabel : ""
                        color: win.s !== null && win.s.exists ? win.statusColor(win.s.status) : Kirigami.Theme.textColor
                    }
                }

                RowLayout {
                    spacing: Kirigami.Units.gridUnit * 2
                    Kirigami.Heading {
                        level: 1
                        text: win.s !== null && win.s.exists ? win.s.percentText : ""
                    }
                    Repeater {
                        model: win.s !== null && win.s.exists ? win.s.stats : []
                        delegate: ColumnLayout {
                            required property var modelData
                            spacing: 0
                            QQC2.Label {
                                text: modelData[0]
                                font: Kirigami.Theme.smallFont
                                opacity: 0.7
                            }
                            QQC2.Label {
                                text: modelData[1]
                            }
                        }
                    }
                }

                Kirigami.SelectableLabel {
                    Layout.fillWidth: true
                    visible: win.s !== null && win.s.exists && win.s.error !== null
                    text: visible ? win.s.error : ""
                    color: Kirigami.Theme.negativeTextColor
                    wrapMode: Text.Wrap
                }
                QQC2.Label {
                    visible: win.s !== null && win.s.exists && win.s.verified
                    text: "✓ Checksum verified"
                    color: Kirigami.Theme.positiveTextColor
                }

                RowLayout {
                    Layout.fillWidth: true
                    QQC2.Label {
                        Layout.fillWidth: true
                        text: win.s !== null && win.s.exists ? win.s.segmentsTitle : ""
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                    }
                    QQC2.Label {
                        text: "Faded = segment done · light edge = write position"
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                    }
                }
                SegmentMap {
                    Layout.fillWidth: true
                    implicitHeight: 26
                    showWriteHead: true
                    download: win.s !== null && win.s.exists ? win.s.segments : null
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.gridUnit

                    // one line per connection
                    QQC2.Frame {
                        Layout.fillWidth: true
                        Layout.alignment: Qt.AlignTop
                        Layout.minimumHeight: Kirigami.Units.gridUnit * 9
                        padding: Kirigami.Units.smallSpacing
                        ColumnLayout {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            spacing: Kirigami.Units.smallSpacing
                            Repeater {
                                model: win.s !== null && win.s.exists ? win.s.connections : []
                                delegate: RowLayout {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    spacing: Kirigami.Units.largeSpacing
                                    QQC2.Label {
                                        text: modelData.number
                                        opacity: 0.7
                                    }
                                    QQC2.Label {
                                        Layout.fillWidth: true
                                        text: modelData.received
                                        elide: Text.ElideRight
                                    }
                                    QQC2.Label {
                                        Layout.preferredWidth: Kirigami.Units.gridUnit * 6
                                        text: modelData.state
                                    }
                                    QQC2.Label {
                                        Layout.preferredWidth: Kirigami.Units.gridUnit * 5
                                        text: modelData.speed
                                        horizontalAlignment: Text.AlignRight
                                    }
                                }
                            }
                        }
                    }

                    ColumnLayout {
                        Layout.preferredWidth: Kirigami.Units.gridUnit * 16
                        Layout.maximumWidth: Kirigami.Units.gridUnit * 16
                        Layout.alignment: Qt.AlignTop
                        spacing: Kirigami.Units.largeSpacing

                        Sparkline {
                            Layout.fillWidth: true
                            graphHeight: 80
                            samples: win.s !== null && win.s.exists ? win.s.speedHistory : []
                            peak: win.s !== null && win.s.exists ? win.s.peak : ""
                        }
                        QQC2.Switch {
                            text: "Limit this download"
                            checked: win.s !== null && win.s.exists && win.s.limitOn
                            onToggled: win.send("limit-on", checked)
                        }
                        RowLayout {
                            visible: win.s !== null && win.s.exists && win.s.limitOn
                            StateField {
                                Layout.preferredWidth: Kirigami.Units.gridUnit * 5
                                value: win.s !== null && win.s.exists ? win.s.limitText : ""
                                onEdited: text => win.send("limit-text", text)
                                onAccepted: win.send("apply-limit")
                            }
                            QQC2.Label {
                                text: "MB/s"
                                opacity: 0.7
                            }
                            QQC2.Button {
                                text: "Apply"
                                onClicked: win.send("apply-limit")
                            }
                        }
                        QQC2.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: win.s !== null && win.s.exists ? win.s.limitError : ""
                            color: Kirigami.Theme.negativeTextColor
                            font: Kirigami.Theme.smallFont
                            wrapMode: Text.Wrap
                        }
                        QQC2.Label {
                            text: "Overrides the per-download setting."
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                        }
                        QQC2.Label {
                            text: "When finished"
                        }
                        StateChoice {
                            Layout.fillWidth: true
                            choices: [
                                { "id": "notify", "name": "Show notification" },
                                { "id": "openFile", "name": "Open file" },
                                { "id": "openFolder", "name": "Show in folder" },
                                { "id": "none", "name": "Do nothing" }
                            ]
                            value: win.s !== null && win.s.exists ? win.s.whenFinished : ""
                            onChosen: id => win.send("when-finished", id)
                        }
                    }
                }
            }
        }
    }
}
