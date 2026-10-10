import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.opendm.app

// The main window. It draws what its view-model (Rust) describes and
// reports what the user does; it decides nothing itself.
Kirigami.ApplicationWindow {
    id: root

    // what to show, from the view-model
    property var s: vm.state.length > 0 ? JSON.parse(vm.state) : null
    // open secondary windows (kept so they aren't collected)
    property var windows: []
    property var progressWindows: ({})
    property var singles: ({})
    readonly property var columns: [
        { "key": "size", "title": "Size", "width": 86, "right": true },
        { "key": "progress", "title": "Progress", "width": 170, "right": false },
        { "key": "speed", "title": "Speed", "width": 96, "right": true },
        { "key": "eta", "title": "ETA", "width": 76, "right": true },
        { "key": "status", "title": "Status", "width": 124, "right": false },
        { "key": "queue", "title": "Queue", "width": 110, "right": false },
        { "key": "added", "title": "Added", "width": 120, "right": false }
    ]

    title: s ? s.title : "Open Download Manager"
    width: 1280
    height: 820
    minimumWidth: 900
    minimumHeight: 560
    visible: false

    function send(action, payload) {
        vm.send(action, JSON.stringify(payload === undefined ? null : payload))
    }

    function command(key) {
        send("command", key)
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

    function showMain() {
        root.show()
        root.raise()
        root.requestActivate()
    }

    function quit() {
        // stops downloads and saves state (waits up to 5 s)
        Backend.shutdown()
        Qt.quit()
    }

    function track(w) {
        windows.push(w)
        w.closing.connect(() => {
            windows = windows.filter(x => x !== w)
            w.destroy()
        })
        w.show()
        w.raise()
        w.requestActivate()
        return w
    }

    function openAdd(params) {
        track(addComponent.createObject(null, { "params": params || {} }))
    }

    function openProgress(id) {
        if (progressWindows[id]) {
            progressWindows[id].raise()
            progressWindows[id].requestActivate()
            return
        }
        const w = track(progressComponent.createObject(null, { "downloadId": id }))
        progressWindows[id] = w
        w.closing.connect(() => delete progressWindows[id])
    }

    function openSingle(kind, component, properties) {
        if (singles[kind]) {
            if (properties && properties.links)
                singles[kind].links = properties.links
            singles[kind].raise()
            singles[kind].requestActivate()
            return
        }
        const w = track(component.createObject(null, properties || {}))
        singles[kind] = w
        w.closing.connect(() => delete singles[kind])
    }

    // a request from a view-model or from the desktop (tray, links, …)
    function handle(what, payload) {
        switch (what) {
        case "show":
        case "main": showMain(); break
        case "add": openAdd(payload); break
        case "progress": openProgress(payload); break
        case "links":
        case "grabber": openSingle("grabber", grabberComponent, payload ? { "links": payload } : null); break
        case "scheduler": openSingle("scheduler", schedulerComponent); break
        case "settings": openSingle("settings", settingsComponent); break
        case "copy": Backend.copy(payload); break
        case "error": errorMessage.text = payload; errorMessage.visible = true; break
        case "quit": quit(); break
        }
    }

    onClosing: close => {
        close.accepted = false
        if (Backend.hidesOnClose())
            root.hide()
        else
            quit()
    }

    Component.onCompleted: {
        Backend.start()
        vm.init("main", "{}")
        if (!Backend.startsHidden())
            showMain()
        Backend.ready()
    }

    onSChanged: if (s) rows.show(s.filter, s.search, s.sort, s.descending)

    ViewModel {
        id: vm
        onRequest: (what, payload) => root.handle(what, payload.length > 0 ? JSON.parse(payload) : null)
    }

    DownloadModel {
        id: rows
    }

    Connections {
        target: Backend
        function onChanged(kind) {
            vm.storeChanged(kind)
            rows.refresh()
        }
        function onShellEvent(kind, json) {
            root.handle(kind, json.length > 0 ? JSON.parse(json) : null)
        }
    }

    // the power countdown
    Timer {
        interval: 1000
        repeat: true
        running: root.s !== null && root.s.power !== null
        onTriggered: root.send("tick")
    }

    Component { id: addComponent; AddWindow { onRequested: (what, payload) => root.handle(what, payload) } }
    Component { id: progressComponent; ProgressWindow { onRequested: (what, payload) => root.handle(what, payload) } }
    Component { id: schedulerComponent; SchedulerWindow { onRequested: (what, payload) => root.handle(what, payload) } }
    Component { id: grabberComponent; GrabberWindow { onRequested: (what, payload) => root.handle(what, payload) } }
    Component { id: settingsComponent; SettingsWindow { onRequested: (what, payload) => root.handle(what, payload) } }

    FileDialog {
        id: importDialog
        title: "Import Link List"
        fileMode: FileDialog.OpenFile
        nameFilters: ["Text files (*.txt *.csv)", "All files (*)"]
        onAccepted: root.send("import", decodeURIComponent(selectedFile.toString().replace(/^file:\/\//, "")))
    }

    FileDialog {
        id: exportDialog
        title: "Export Link List"
        fileMode: FileDialog.SaveFile
        currentFile: "downloads.txt"
        nameFilters: ["Text files (*.txt)"]
        onAccepted: root.send("export", decodeURIComponent(selectedFile.toString().replace(/^file:\/\//, "")))
    }

    // "Delete N downloads?"
    Kirigami.PromptDialog {
        id: deleteDialog
        visible: root.s !== null && root.s.dialog !== null && root.s.dialog.kind === "delete"
        title: visible ? root.s.dialog.title : ""
        subtitle: visible ? root.s.dialog.text : ""
        standardButtons: Kirigami.Dialog.Cancel
        customFooterActions: [
            Kirigami.Action {
                text: "Remove from List"
                onTriggered: root.send("dialog-accept", "list")
            },
            Kirigami.Action {
                text: "Remove and Delete Files"
                icon.name: "edit-delete"
                onTriggered: root.send("dialog-accept", "files")
            }
        ]
        onRejected: root.send("dialog-cancel")
    }

    // one line of text: a fresh link, or a custom speed limit
    Kirigami.PromptDialog {
        id: textDialog
        visible: root.s !== null && root.s.dialog !== null && root.s.dialog.kind === "text"
        title: visible ? root.s.dialog.title : ""
        subtitle: visible ? root.s.dialog.text : ""
        standardButtons: Kirigami.Dialog.Cancel
        customFooterActions: [
            Kirigami.Action {
                text: textDialog.visible ? root.s.dialog.accept : ""
                onTriggered: root.send("dialog-accept", "")
            }
        ]
        onRejected: root.send("dialog-cancel")
        QQC2.TextField {
            Layout.fillWidth: true
            text: textDialog.visible ? root.s.dialog.value : ""
            onTextEdited: root.send("dialog-text", text)
            onAccepted: root.send("dialog-accept", "")
        }
    }

    // the right-click menu of the list, for the selection
    QQC2.Menu {
        id: rowMenu
        readonly property var m: root.s ? root.s.menu : null
        QQC2.MenuItem { text: "Open"; enabled: rowMenu.m && rowMenu.m.canOpen; onTriggered: root.command("open") }
        QQC2.MenuItem { text: "Show in Folder"; enabled: rowMenu.m && rowMenu.m.one; onTriggered: root.command("reveal") }
        QQC2.MenuItem { text: "Show Progress Window"; enabled: rowMenu.m && rowMenu.m.canProgress; onTriggered: root.command("progress") }
        QQC2.MenuSeparator {}
        QQC2.MenuItem { text: "Resume"; enabled: rowMenu.m && rowMenu.m.canResume; onTriggered: root.command("resume") }
        QQC2.MenuItem { text: "Pause"; enabled: rowMenu.m && rowMenu.m.canPause; onTriggered: root.command("pause") }
        QQC2.MenuItem { text: "Redownload"; enabled: rowMenu.m && rowMenu.m.one; onTriggered: root.command("redownload") }
        QQC2.MenuItem { text: "Refresh Link…"; enabled: rowMenu.m && rowMenu.m.canProgress; onTriggered: root.command("refresh") }
        QQC2.MenuItem { text: "Copy Link"; enabled: rowMenu.m && rowMenu.m.one; onTriggered: root.command("copy") }
        QQC2.MenuSeparator {}
        QQC2.Menu {
            id: queueMenu
            title: "Move to Queue"
            Instantiator {
                model: rowMenu.m ? rowMenu.m.queues : []
                delegate: QQC2.MenuItem {
                    required property var modelData
                    text: modelData.name
                    onTriggered: root.command("queue:" + modelData.id)
                }
                onObjectAdded: (index, object) => queueMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => queueMenu.removeItem(object)
            }
            QQC2.MenuSeparator {}
            QQC2.MenuItem { text: "Remove from Queue"; onTriggered: root.command("queue:") }
        }
        QQC2.Menu {
            id: categoryMenu
            title: "Category"
            Instantiator {
                model: rowMenu.m ? rowMenu.m.categories : []
                delegate: QQC2.MenuItem {
                    required property var modelData
                    text: modelData.name
                    onTriggered: root.command("category:" + modelData.id)
                }
                onObjectAdded: (index, object) => categoryMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => categoryMenu.removeItem(object)
            }
        }
        QQC2.Menu {
            id: alwaysMenu
            title: rowMenu.m && rowMenu.m.extension.length > 0 ? "Always Put ." + rowMenu.m.extension + " Files In" : "Always Put This Type In"
            enabled: rowMenu.m !== null && rowMenu.m.extension.length > 0
            Instantiator {
                model: rowMenu.m ? rowMenu.m.categories : []
                delegate: QQC2.MenuItem {
                    required property var modelData
                    text: modelData.name
                    onTriggered: root.command("always:" + modelData.id)
                }
                onObjectAdded: (index, object) => alwaysMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => alwaysMenu.removeItem(object)
            }
        }
        QQC2.MenuSeparator {}
        QQC2.MenuItem {
            text: rowMenu.m && rowMenu.m.count > 1 ? "Delete " + rowMenu.m.count + " Downloads…" : "Delete…"
            icon.name: "edit-delete"
            onTriggered: root.command("delete")
        }
    }

    pageStack.initialPage: Kirigami.Page {
        id: page
        title: "Downloads"
        padding: 0

        actions: [
            Kirigami.Action {
                text: "Add URL"
                icon.name: "list-add"
                shortcut: StandardKey.New
                onTriggered: root.command("add")
            },
            Kirigami.Action {
                text: "Resume"
                icon.name: "media-playback-start"
                shortcut: "Ctrl+R"
                enabled: root.s !== null && root.s.menu.canResume
                onTriggered: root.command("resume")
            },
            Kirigami.Action {
                text: "Pause"
                icon.name: "media-playback-pause"
                shortcut: "Ctrl+."
                enabled: root.s !== null && root.s.menu.canPause
                onTriggered: root.command("pause")
            },
            Kirigami.Action {
                text: "Delete"
                icon.name: "edit-delete"
                enabled: root.s !== null && root.s.menu.count > 0
                onTriggered: root.command("delete")
            },
            Kirigami.Action {
                text: "Queue"
                icon.name: "view-list-details"
                Kirigami.Action { text: "Start Queue"; icon.name: "media-playback-start"; onTriggered: root.command("start-queue") }
                Kirigami.Action { text: "Stop Queue"; icon.name: "media-playback-stop"; onTriggered: root.command("stop-queue") }
                Kirigami.Action { text: "Queues && Scheduler…"; icon.name: "view-calendar"; shortcut: "Ctrl+J"; onTriggered: root.command("scheduler") }
            },
            Kirigami.Action {
                text: "Grabber"
                icon.name: "internet-web-browser"
                shortcut: "Ctrl+Shift+G"
                onTriggered: root.command("grabber")
            },
            Kirigami.Action {
                text: root.s ? root.s.limitLabel : "No Limit"
                icon.name: "speedometer"
                Kirigami.Action { text: "1 MB/s"; onTriggered: root.command("limit:1") }
                Kirigami.Action { text: "2 MB/s"; onTriggered: root.command("limit:2") }
                Kirigami.Action { text: "5 MB/s"; onTriggered: root.command("limit:5") }
                Kirigami.Action { text: "10 MB/s"; onTriggered: root.command("limit:10") }
                Kirigami.Action { text: "25 MB/s"; onTriggered: root.command("limit:25") }
                Kirigami.Action { text: "50 MB/s"; onTriggered: root.command("limit:50") }
                Kirigami.Action { text: "100 MB/s"; onTriggered: root.command("limit:100") }
                Kirigami.Action { text: "Custom…"; onTriggered: root.command("limit-custom") }
                Kirigami.Action {
                    text: root.s !== null && root.s.limitOn ? "Turn Off Limit" : "Turn On Limit"
                    shortcut: "Ctrl+Alt+L"
                    onTriggered: root.command("limit-toggle")
                }
            },
            Kirigami.Action {
                text: "Details"
                icon.name: "view-split-top-bottom"
                checkable: true
                checked: root.s !== null && root.s.detailsOn
                onTriggered: root.command("details")
            },
            Kirigami.Action {
                text: "Add Batch Download…"
                icon.name: "list-add"
                shortcut: "Ctrl+Shift+N"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.command("add-batch")
            },
            Kirigami.Action {
                text: "Pause All"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.command("pause-all")
            },
            Kirigami.Action {
                text: "Delete Completed"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.command("delete-completed")
            },
            Kirigami.Action {
                text: "Import Link List…"
                icon.name: "document-import"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: importDialog.open()
            },
            Kirigami.Action {
                text: "Export Link List…"
                icon.name: "document-export"
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: exportDialog.open()
            },
            Kirigami.Action {
                text: "Settings…"
                icon.name: "configure"
                shortcut: StandardKey.Preferences
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.command("settings")
            },
            Kirigami.Action {
                text: "Quit"
                icon.name: "application-exit"
                shortcut: StandardKey.Quit
                displayHint: Kirigami.DisplayHint.AlwaysHide
                onTriggered: root.quit()
            }
        ]

        // links dragged from a browser
        DropArea {
            anchors.fill: parent
            onDropped: drop => {
                if (drop.hasUrls)
                    root.send("links", drop.urls.join("\n"))
                else if (drop.hasText)
                    root.send("links", drop.text)
            }
        }

        RowLayout {
            anchors.fill: parent
            spacing: 0

            // ---------------------------------------------------- sidebar
            QQC2.ScrollView {
                Layout.fillHeight: true
                Layout.preferredWidth: Kirigami.Units.gridUnit * 13
                QQC2.ScrollBar.horizontal.policy: QQC2.ScrollBar.AlwaysOff

                ColumnLayout {
                    width: Kirigami.Units.gridUnit * 13
                    spacing: 0

                    Kirigami.SearchField {
                        Layout.fillWidth: true
                        Layout.margins: Kirigami.Units.smallSpacing
                        placeholderText: "Search downloads, URLs, folders"
                        onTextChanged: root.send("search", text)
                    }

                    Repeater {
                        model: root.s ? root.s.sidebar : []
                        delegate: Loader {
                            id: entry
                            required property var modelData
                            Layout.fillWidth: true
                            sourceComponent: modelData.header !== undefined ? sectionHeader : sectionItem

                            Component {
                                id: sectionHeader
                                Kirigami.ListSectionHeader {
                                    text: entry.modelData.header
                                }
                            }
                            Component {
                                id: sectionItem
                                SidebarItem {
                                    text: entry.modelData.title
                                    iconName: entry.modelData.icon
                                    count: entry.modelData.count
                                    checked: root.s !== null && root.s.filter === entry.modelData.key
                                    onClicked: root.send("nav", entry.modelData.key)
                                }
                            }
                        }
                    }
                }
            }

            Kirigami.Separator {
                Layout.fillHeight: true
            }

            // ---------------------------------------------------- content
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 0

                Kirigami.InlineMessage {
                    Layout.fillWidth: true
                    type: Kirigami.MessageType.Warning
                    visible: root.s !== null && root.s.power !== null
                    text: visible ? root.s.power : ""
                    actions: [
                        Kirigami.Action {
                            text: "Cancel"
                            onTriggered: root.command("cancel-power")
                        }
                    ]
                }
                Kirigami.InlineMessage {
                    Layout.fillWidth: true
                    type: Kirigami.MessageType.Error
                    visible: root.s !== null && root.s.startError.length > 0
                    text: visible ? "The download engine failed to start: " + root.s.startError : ""
                }
                Kirigami.InlineMessage {
                    id: errorMessage
                    Layout.fillWidth: true
                    type: Kirigami.MessageType.Error
                    showCloseButton: true
                    visible: false
                }

                // column headers: click to sort
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: Kirigami.Units.largeSpacing
                    Layout.rightMargin: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.smallSpacing

                    QQC2.ToolButton {
                        Layout.fillWidth: true
                        text: "Name" + (root.s !== null && root.s.sort === "name" ? (root.s.descending ? " ↓" : " ↑") : "")
                        onClicked: root.send("sort", "name")
                    }
                    Repeater {
                        model: root.columns
                        delegate: QQC2.ToolButton {
                            required property var modelData
                            Layout.preferredWidth: modelData.width
                            text: modelData.title + (root.s !== null && root.s.sort === modelData.key ? (root.s.descending ? " ↓" : " ↑") : "")
                            onClicked: root.send("sort", modelData.key)
                        }
                    }
                }
                Kirigami.Separator {
                    Layout.fillWidth: true
                }

                QQC2.ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true

                    ListView {
                        id: list
                        model: rows
                        clip: true
                        reuseItems: true
                        keyNavigationEnabled: false

                        Keys.onDeletePressed: root.command("delete")

                        Kirigami.PlaceholderMessage {
                            anchors.centerIn: parent
                            width: parent.width - Kirigami.Units.gridUnit * 4
                            visible: root.s !== null && root.s.empty.length > 0
                            icon.name: "folder-download-symbolic"
                            text: visible ? root.s.empty : ""
                            explanation: root.s !== null && root.s.emptyHint ? "Add a link, or drag one from your browser onto this window." : ""
                            helpfulAction: Kirigami.Action {
                                text: "Add URL…"
                                icon.name: "list-add"
                                onTriggered: root.command("add")
                            }
                        }

                        delegate: QQC2.ItemDelegate {
                            id: row
                            required property string id
                            required property string name
                            required property string size
                            required property int percent
                            required property string speed
                            required property string eta
                            required property string statusLabel
                            required property string status
                            required property string queue
                            required property string added
                            required property string category

                            width: ListView.view.width
                            highlighted: root.s !== null && root.s.selected.indexOf(row.id) >= 0
                            QQC2.ToolTip.text: row.name
                            QQC2.ToolTip.visible: hovered && nameLabel.truncated
                            QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay

                            TapHandler {
                                acceptedButtons: Qt.LeftButton
                                onTapped: (point, button) => {
                                    list.forceActiveFocus()
                                    root.send("select", {
                                        "id": row.id,
                                        "ctrl": (point.modifiers & Qt.ControlModifier) !== 0,
                                        "shift": (point.modifiers & Qt.ShiftModifier) !== 0
                                    })
                                }
                                onDoubleTapped: root.send("activate", row.id)
                            }
                            TapHandler {
                                acceptedButtons: Qt.RightButton
                                onTapped: {
                                    root.send("context", row.id)
                                    rowMenu.popup()
                                }
                            }

                            contentItem: RowLayout {
                                spacing: Kirigami.Units.smallSpacing

                                Kirigami.Icon {
                                    source: {
                                        switch (row.category) {
                                        case "video": return "video-x-generic"
                                        case "music": return "audio-x-generic"
                                        case "documents": return "x-office-document"
                                        case "compressed": return "application-x-archive"
                                        case "programs": return "application-x-executable"
                                        }
                                        return "text-x-generic"
                                    }
                                    implicitWidth: Kirigami.Units.iconSizes.small
                                    implicitHeight: Kirigami.Units.iconSizes.small
                                }
                                QQC2.Label {
                                    id: nameLabel
                                    Layout.fillWidth: true
                                    text: row.name
                                    elide: Text.ElideMiddle
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[0].width
                                    text: row.size
                                    horizontalAlignment: Text.AlignRight
                                }
                                RowLayout {
                                    Layout.preferredWidth: root.columns[1].width
                                    Layout.maximumWidth: root.columns[1].width
                                    spacing: Kirigami.Units.smallSpacing
                                    QQC2.ProgressBar {
                                        Layout.fillWidth: true
                                        from: 0
                                        to: 100
                                        value: row.percent
                                    }
                                    QQC2.Label {
                                        Layout.preferredWidth: Kirigami.Units.gridUnit * 2
                                        text: row.percent + "%"
                                        horizontalAlignment: Text.AlignRight
                                    }
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[2].width
                                    text: row.speed
                                    horizontalAlignment: Text.AlignRight
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[3].width
                                    text: row.eta
                                    horizontalAlignment: Text.AlignRight
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[4].width
                                    text: "● " + row.statusLabel
                                    color: root.statusColor(row.status)
                                    elide: Text.ElideRight
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[5].width
                                    text: row.queue
                                    opacity: 0.7
                                    elide: Text.ElideRight
                                }
                                QQC2.Label {
                                    Layout.preferredWidth: root.columns[6].width
                                    text: row.added
                                    opacity: 0.7
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }
                }

                // ------------------------------------------------ details
                Kirigami.Separator {
                    Layout.fillWidth: true
                    visible: detailsPane.visible
                }
                ColumnLayout {
                    id: detailsPane
                    readonly property var d: root.s ? root.s.details : null
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: d !== null
                    spacing: Kirigami.Units.largeSpacing

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.largeSpacing
                        Kirigami.Icon {
                            source: detailsPane.d ? detailsPane.d.icon : ""
                            implicitWidth: Kirigami.Units.iconSizes.medium
                            implicitHeight: Kirigami.Units.iconSizes.medium
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            Kirigami.SelectableLabel {
                                Layout.fillWidth: true
                                text: detailsPane.d ? detailsPane.d.name : ""
                                font.bold: true
                            }
                            Kirigami.SelectableLabel {
                                Layout.fillWidth: true
                                text: detailsPane.d ? detailsPane.d.url : ""
                                font: Kirigami.Theme.smallFont
                                opacity: 0.7
                            }
                        }
                        QQC2.Label {
                            text: detailsPane.d ? "● " + detailsPane.d.statusLabel : ""
                            color: detailsPane.d ? root.statusColor(detailsPane.d.status) : Kirigami.Theme.textColor
                        }
                        QQC2.Button {
                            visible: detailsPane.d !== null && detailsPane.d.retry
                            text: "Retry"
                            onClicked: root.command("detail-resume:" + detailsPane.d.id)
                        }
                        QQC2.Button {
                            text: detailsPane.d ? detailsPane.d.primary.label : ""
                            highlighted: true
                            onClicked: root.command("detail-" + detailsPane.d.primary.action + ":" + detailsPane.d.id)
                        }
                        QQC2.Button {
                            icon.name: "document-open-folder"
                            text: "Show in Folder"
                            display: QQC2.AbstractButton.IconOnly
                            QQC2.ToolTip.text: text
                            QQC2.ToolTip.visible: hovered
                            onClicked: root.command("detail-reveal:" + detailsPane.d.id)
                        }
                        QQC2.Button {
                            icon.name: "edit-copy"
                            text: "Copy Link"
                            display: QQC2.AbstractButton.IconOnly
                            QQC2.ToolTip.text: text
                            QQC2.ToolTip.visible: hovered
                            onClicked: root.command("detail-copy:" + detailsPane.d.id)
                        }
                    }
                    Kirigami.SelectableLabel {
                        Layout.fillWidth: true
                        visible: detailsPane.d !== null && detailsPane.d.error !== null
                        text: visible ? detailsPane.d.error : ""
                        color: Kirigami.Theme.negativeTextColor
                        wrapMode: Text.Wrap
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.gridUnit * 2
                        GridLayout {
                            columns: 2
                            columnSpacing: Kirigami.Units.largeSpacing
                            rowSpacing: Kirigami.Units.smallSpacing
                            Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                            Repeater {
                                model: detailsPane.d ? detailsPane.d.facts : []
                                delegate: Item {
                                    required property var modelData
                                    Layout.columnSpan: 2
                                    Layout.fillWidth: true
                                    implicitHeight: fact.implicitHeight
                                    RowLayout {
                                        id: fact
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        spacing: Kirigami.Units.largeSpacing
                                        QQC2.Label {
                                            Layout.preferredWidth: Kirigami.Units.gridUnit * 6
                                            text: modelData[0]
                                            opacity: 0.7
                                        }
                                        QQC2.Label {
                                            Layout.fillWidth: true
                                            text: modelData[1]
                                            elide: Text.ElideMiddle
                                        }
                                    }
                                }
                            }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing
                            RowLayout {
                                Layout.fillWidth: true
                                QQC2.Label {
                                    Layout.fillWidth: true
                                    text: "Segments"
                                    font: Kirigami.Theme.smallFont
                                    opacity: 0.7
                                }
                                QQC2.Label {
                                    text: detailsPane.d ? detailsPane.d.percent + "%" : ""
                                    font: Kirigami.Theme.smallFont
                                    opacity: 0.7
                                }
                            }
                            SegmentMap {
                                Layout.fillWidth: true
                                download: detailsPane.d ? detailsPane.d.segments : null
                            }
                            Sparkline {
                                Layout.fillWidth: true
                                samples: detailsPane.d ? detailsPane.d.speedHistory : []
                                peak: detailsPane.d ? detailsPane.d.peak : ""
                                graphHeight: 52
                            }
                        }
                    }
                }

                // --------------------------------------------- status bar
                Kirigami.Separator {
                    Layout.fillWidth: true
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.smallSpacing
                    Layout.leftMargin: Kirigami.Units.largeSpacing
                    Layout.rightMargin: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.gridUnit
                    opacity: 0.8

                    QQC2.Label { text: root.s ? root.s.status.summary : ""; font: Kirigami.Theme.smallFont }
                    QQC2.Label { text: root.s ? root.s.status.speed : ""; font: Kirigami.Theme.smallFont }
                    QQC2.Label { text: root.s ? root.s.status.limit : ""; font: Kirigami.Theme.smallFont }
                    QQC2.Label {
                        text: root.s ? (root.s.status.ext.state === "idle" ? "○ " : "● ") + root.s.status.ext.text : ""
                        font: Kirigami.Theme.smallFont
                        color: root.s ? root.statusColor(root.s.status.ext.state) : Kirigami.Theme.textColor
                        QQC2.ToolTip.text: root.s ? root.s.status.ext.tooltip : ""
                        QQC2.ToolTip.visible: extHover.hovered && QQC2.ToolTip.text.length > 0
                        HoverHandler { id: extHover }
                    }
                    Item { Layout.fillWidth: true }
                    QQC2.Label { text: root.s ? root.s.status.free : ""; font: Kirigami.Theme.smallFont }
                }
            }
        }
    }
}
