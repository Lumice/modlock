pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Dialogs
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

// Settings and maintenance in one place. Actions reuse the existing
// reviewed workflows; nothing here changes the game directly.
Item {
    id: root
    objectName: "settingsPage"
    required property LibraryModel library
    readonly property bool popupOpen: storageLoader.active || maintenanceLoader.active || gameFolder.visible || folderConfirmation.visible
    property bool advancedToolsOpen: false
    property string pendingGameFolder: ""
    function chooseGameFolder(path) {
        if (root.library.nativeMode) return root.library.useGameFolder(path)
        if (!root.library.createLibrary()) return false
        root.pendingGameFolder = path
        return true
    }
    function finishGameFolderChoice() {
        if (root.pendingGameFolder && root.library.canEditProfiles) {
            const path = root.pendingGameFolder
            root.pendingGameFolder = ""
            root.library.useGameFolder(path)
        }
    }
    Connections {
        target: root.library
        function onNativeChanged() { Qt.callLater(root.finishGameFolderChoice) }
        function onBusyChanged() { Qt.callLater(root.finishGameFolderChoice) }
        function onLoadingChanged() { Qt.callLater(root.finishGameFolderChoice) }
    }
    signal launchSettingsRequested()
    signal applyRequested()
    signal restoreRequested()
    signal orderRequested()
    signal historyRequested()
    signal sourceRequested()
    signal profilesRequested()
    signal installedRequested()
    signal bisectRequested()

    function gameSetupText(state) {
        if (state.recoveryNeedsAttention) return qsTr("An unfinished change needs recovery.")
        if (state.busy || state.loading || (state.recoveryRequired && !state.recoveryNeedsAttention)) return ""
        if (state.externalGameinfoChange) return qsTr("Game setup changed outside Modlock.")
        if (state.appliedProfileId) return qsTr("A restore point from the previous Apply is saved.")
        if (state.canRestore) return qsTr("Modlock has a verified restore point.")
        if (state.gamePath) return qsTr("No verified restore point. Apply checks the game setup before changing it.")
        return qsTr("Choose your Deadlock folder to check the game setup.")
    }
    function catalogProgressText(progress) {
        const title = progress.stage === "collections" ? qsTr("Loading collections") : qsTr("Loading mod list")
        if (progress.total === null || progress.total === undefined) return qsTr("%1: %2 loaded").arg(title).arg(progress.completed || "0")
        return qsTr("%1: %2 of %3").arg(title).arg(progress.completed || "0").arg(progress.total)
    }
    function folderUrl(path) { return "file://" + path.split("/").map(encodeURIComponent).join("/") }

    component Section: Rectangle {
        id: section
        property string title
        property string description
        default property alias content: body.data
        Layout.fillWidth: true
        implicitHeight: column.implicitHeight + Theme.spacing32
        radius: Theme.radius
        color: Theme.surfaceLow
        border.color: Theme.rule
        ColumnLayout {
            id: column
            anchors.fill: parent
            anchors.margins: Theme.spacing16
            spacing: Theme.spacing12
            Label { text: section.title; font.pointSize: Theme.title; font.weight: Font.DemiBold }
            Label {
                Layout.fillWidth: true
                visible: section.description !== ""
                text: section.description
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            ColumnLayout { id: body; Layout.fillWidth: true; spacing: Theme.spacing8 }
        }
    }
    component PathRow: RowLayout {
        id: pathRow
        property string label
        property string path
        property string emptyText
        property string changeObjectName: "changeStorageLocation"
        property bool changeVisible: false
        property bool changeEnabled: false
        signal changeRequested()
        Layout.fillWidth: true
        spacing: Theme.spacing12
        Label { text: pathRow.label; color: Theme.secondary; Layout.preferredWidth: 160 }
        Label {
            Layout.fillWidth: true
            text: pathRow.path || pathRow.emptyText
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
            ToolTip.visible: pathHover.hovered
            ToolTip.text: text
            HoverHandler { id: pathHover }
            color: pathRow.path ? Theme.text : Theme.faint
        }
        Button {
            objectName: pathRow.changeVisible ? pathRow.changeObjectName : ""
            visible: pathRow.changeVisible
            enabled: pathRow.changeEnabled
            text: qsTr("Change…")
            onClicked: pathRow.changeRequested()
        }
        Button {
            text: qsTr("Open")
            flat: true
            icon.source: Theme.icon("folder")
            enabled: pathRow.path !== ""
            onClicked: Qt.openUrlExternally(root.folderUrl(pathRow.path))
        }
    }

    function storageSummary(current, busy) {
        if (current?.unavailable) return qsTr("Storage drive is unavailable. Reconnect it before using library files.")
        if (current?.bytes == null || current.freeBytes == null) return busy ? qsTr("Checking storage…") : ""
        return qsTr("Folder size: %1 GiB · Free on drive: %2 GiB")
            .arg((Number(current.bytes) / 1073741824).toFixed(2))
            .arg((Number(current.freeBytes) / 1073741824).toFixed(2))
    }
    StorageController { id: storage; objectName: "settingsStorageController"; library: root.library }
    Component.onCompleted: { if (root.library.nativeMode) storage.scan() }
    Loader {
        id: storageLoader
        active: false
        sourceComponent: StorageDialog { library: root.library; controller: storage; onClosed: storageLoader.active = false }
    }
    Loader {
        id: maintenanceLoader
        active: false
        sourceComponent: MaintenanceDialog { library: root.library; onClosed: maintenanceLoader.active = false; onApplyRequested: root.applyRequested() }
    }

    FolderDialog {
        id: gameFolder
        title: qsTr("Choose the Steam Deadlock folder")
        onAccepted: folderConfirmation.open()
    }
    Dialog {
        id: folderConfirmation
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(520, parent ? parent.width - 40 : 520)
        title: qsTr("Use this game folder")
        modal: true
        standardButtons: Dialog.Cancel
        contentItem: Label {
            text: root.library.localPath(gameFolder.selectedFolder) + "\n" + qsTr("This folder will be saved for your mods and game settings. Existing restore points stay with the previous folder.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        footer: DialogButtonBox {
            Button { text: qsTr("Cancel"); DialogButtonBox.buttonRole: DialogButtonBox.RejectRole }
            Button { text: qsTr("Use this game folder"); onClicked: { root.chooseGameFolder(root.library.localPath(gameFolder.selectedFolder)); folderConfirmation.close() } }
            onRejected: folderConfirmation.close()
        }
    }
    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            width: Math.min(parent.width - 48, 760)
            x: 24
            spacing: Theme.spacing16
            Item { Layout.preferredHeight: Theme.spacing8 }
            SectionHeading { text: qsTr("Settings"); pointSize: Theme.heading }

            Section {
                title: qsTr("Game")
                description: root.library.gamePath ? ""
                    : root.library.detectedGamePath ? qsTr("Found in your Steam library. You can choose a different Deadlock folder below.")
                    : qsTr("Deadlock wasn't found. Install it with Steam, then choose the Deadlock folder containing game/citadel. You can collect mods before installing the game.")
                Label { objectName: "gameSetupStatus"; Layout.fillWidth: true; text: root.gameSetupText(root.library); visible: text.length > 0; wrapMode: Text.Wrap }
                PathRow { label: qsTr("Deadlock folder"); path: root.library.gamePath || root.library.detectedGamePath; emptyText: qsTr("Not found in your Steam libraries"); changeVisible: true; changeObjectName: "chooseGameFolderButton"; changeEnabled: root.library.canEditProfiles || root.library.canCreateLibrary; onChangeRequested: gameFolder.open() }
                PathRow {
                    label: qsTr("Game config folder")
                    readonly property string game: root.library.gamePath || root.library.detectedGamePath
                    path: game ? game + "/game/citadel/cfg" : ""
                    emptyText: qsTr("Found once the Deadlock folder is known")
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Theme.spacing8
                    Button { text: qsTr("Check and apply setup…"); enabled: root.library.canPreviewApply; onClicked: root.applyRequested() }
                    Button { text: qsTr("Restore previous setup…"); enabled: root.library.canRestore && !root.library.busy; onClicked: root.restoreRequested() }
                }
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Check for crashes after launch")
                    description: qsTr("Ask about early exits and show what changed since your last normal session.")
                    Switch {
                        objectName: "crashCheckSwitch"
                        Accessible.name: qsTr("Check for crashes after launch")
                        checked: root.library.launch.health.enabled
                        onToggled: root.library.launch.health.enabled = checked
                    }
                }
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Fix mods automatically after game updates")
                    description: qsTr("Choose verified fixes after an update. They are applied with your mod changes.")
                    Switch {
                        objectName: "fixAutomaticallySwitch"
                        Accessible.name: qsTr("Fix mods automatically after game updates")
                        checked: root.library.fixAutomatically
                        enabled: root.library.canSetEnabled
                        onToggled: { root.library.setFixAutomatically(checked); checked = Qt.binding(() => root.library.fixAutomatically) }
                    }
                }
            }

            Section {
                title: qsTr("Library")
                description: qsTr("Your mods, profiles and saved setups.")
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Apply changes automatically")
                    description: qsTr("Safe mod switches, choices, order and updates are applied automatically. Changes needing attention still open a review.")
                    Switch {
                        objectName: "automaticApplySwitch"
                        Accessible.name: qsTr("Apply changes automatically")
                        checked: Preferences.automaticApply
                        onToggled: { Preferences.automaticApply = checked; checked = Qt.binding(() => Preferences.automaticApply) }
                    }
                }
                PathRow {
                    objectName: "storageLocationRow"
                    label: qsTr("Storage location")
                    path: storage.result.current?.path || root.library.dataDir
                    emptyText: qsTr("Not created yet")
                    changeVisible: true
                    changeEnabled: root.library.nativeMode && !root.library.busy && !storage.busy
                    onChangeRequested: { storageLoader.active = true; storage.scan() }
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 152
                    font.pointSize: Theme.small
                    wrapMode: Text.Wrap
                    visible: text !== ""
                    text: root.storageSummary(storage.result.current, storage.busy)
                    color: Theme.secondary
                }
                Label { Layout.fillWidth: true; visible: !!storage.error; text: Theme.friendlyError(storage.error); textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.warning }
                PathRow {
                    objectName: "libraryFolderRow"
                    label: qsTr("App data folder")
                    path: root.library.dataDir
                    emptyText: qsTr("Not created yet")
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!storage.result.current?.path && storage.result.current.path !== root.library.dataDir
                    text: qsTr("Mod files use the storage location; profiles and library records stay in the library folder.")
                    color: Theme.secondary
                    font.pointSize: Theme.small
                    wrapMode: Text.Wrap
                }
            }

            Section {
                title: qsTr("Caches")
                description: qsTr("Refresh saved catalog information or review unused downloads. Your mods and profiles are kept.")
                Button {
                    objectName: "clearCatalogCache"
                    text: qsTr("Clear catalog cache and resync")
                    enabled: root.library.catalog.canClearCache && !root.library.busy
                    onClicked: root.library.catalog.clearCacheAndResync()
                }
                Label { Layout.fillWidth: true; text: root.library.catalog.errorSummary || root.library.catalog.workNotice; textFormat: Text.PlainText; wrapMode: Text.Wrap; visible: text.length > 0 }
                Label { Layout.fillWidth: true; text: root.library.busy ? qsTr("Wait for the current library change to finish.") : root.library.catalog.clearCacheReason; visible: text.length > 0; textFormat: Text.PlainText; color: Theme.secondary; wrapMode: Text.Wrap }
                Label { Layout.fillWidth: true; visible: root.library.catalog.busy && Object.keys(root.library.catalog.syncProgress).length > 0; text: root.catalogProgressText(root.library.catalog.syncProgress); textFormat: Text.PlainText; wrapMode: Text.Wrap }
                ProgressBar { Layout.fillWidth: true; visible: root.library.catalog.busy && Object.keys(root.library.catalog.syncProgress).length > 0; indeterminate: !Number(root.library.catalog.syncProgress.total); from: 0; to: Math.max(1, Number(root.library.catalog.syncProgress.total || 1)); value: Number(root.library.catalog.syncProgress.completed || 0) }
                Button { objectName: "clearDownloadCache"; text: qsTr("Review unused downloads…"); enabled: root.library.nativeMode && !root.library.busy; onClicked: maintenanceLoader.active = true }
            }

            Section {
                title: qsTr("Appearance")
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Colours")
                    SegmentedControl {
                        objectName: "settingsColours"
                        model: [{text: qsTr("Modlock"), value: "modlock"}, {text: qsTr("Desktop theme"), value: "desktop"}]
                        currentValue: Preferences.colourMode
                        onActivated: value => Preferences.colourMode = value
                    }
                }
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Reduce motion")
                    Switch { objectName: "reduceMotionSwitch"; Accessible.name: qsTr("Reduce motion"); checked: Preferences.reduceMotion; onToggled: Preferences.reduceMotion = checked }
                }
            }

            Section {
                title: qsTr("Display")
                description: qsTr("Previews of mods marked NSFW, or without a rating, are covered until you choose Show.")
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Hide NSFW mods in Browse")
                    Switch { Accessible.name: qsTr("Hide NSFW mods in Browse"); checked: Preferences.hideNsfwMods; onToggled: Preferences.hideNsfwMods = checked }
                }
                SettingRow {
                    Layout.fillWidth: true
                    title: qsTr("Show NSFW previews without asking")
                    description: qsTr("Pictures of these mods load and show like any other. Only changes this app’s display.")
                    Switch {
                        objectName: "showMaturePreviewsSwitch"
                        checked: Preferences.showMaturePreviews
                        onToggled: Preferences.showMaturePreviews = checked
                        Accessible.name: qsTr("Show NSFW previews without asking")
                    }
                }
            }

            Section {
                title: qsTr("Advanced tools")
                description: qsTr("Repair, storage, profile and font tools.")
                Button {
                    text: root.advancedToolsOpen ? qsTr("Hide advanced tools") : qsTr("Show advanced tools")
                    checkable: true
                    checked: root.advancedToolsOpen
                    onToggled: root.advancedToolsOpen = checked
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    visible: root.advancedToolsOpen
                    spacing: Theme.spacing12
                    Flow {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        Button { text: qsTr("Launch settings…"); icon.source: Theme.icon("settings"); enabled: root.library.nativeMode; onClicked: root.launchSettingsRequested() }
                        Button { text: qsTr("Mods already in the game…"); icon.source: Theme.icon("search"); visible: root.library.canScanInstalledMods; onClicked: root.installedRequested() }
                        Button { text: qsTr("Find a broken mod…"); icon.source: Theme.icon("search"); enabled: root.library.canStartBisect; onClicked: root.bisectRequested() }
                        Button { text: qsTr("Play without mods"); icon.source: Theme.icon("play"); enabled: root.library.launch.canLaunch; onClicked: root.library.launch.launchActive("vanilla") }
                    }
                    Flow {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        Button { text: qsTr("Apply changes…"); icon.source: Theme.icon("check"); enabled: root.library.canPreviewApply; onClicked: root.applyRequested() }
                        Button { text: qsTr("Restore previous setup…"); icon.source: Theme.icon("undo"); enabled: root.library.canRestore; onClicked: root.restoreRequested() }
                        Button { text: qsTr("Load order…"); icon.source: Theme.icon("sort"); enabled: root.library.options.available; onClicked: root.orderRequested() }
                        Button { text: qsTr("Profiles…"); icon.source: Theme.icon("user"); enabled: root.library.canEditProfiles; onClicked: root.profilesRequested() }
                        Button { text: qsTr("Import history…"); icon.source: Theme.icon("package"); enabled: root.library.nativeMode; onClicked: root.historyRequested() }
                    }
                    Label {
                        objectName: "previousFolderRestoreReason"
                        Layout.fillWidth: true
                        readonly property string reason: root.library.warnings.filter(w => w.startsWith("This restore point belongs to the previous game folder (")).join("\n")
                        visible: reason.length > 0
                        text: reason
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacing4
                        Button { text: qsTr("Import from the old Mod Manager…"); icon.source: Theme.icon("folder"); onClicked: root.sourceRequested() }
                        Label { Layout.fillWidth: true; text: qsTr("Import mods and profiles from a library saved by the old Mod Manager."); color: Theme.secondary; wrapMode: Text.Wrap; font.pointSize: Theme.small }
                    }
                    Flow {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        Button { objectName: "checkModFolders"; text: qsTr("Check folders and free up space…"); enabled: root.library.nativeMode && !root.library.busy; onClicked: maintenanceLoader.active = true }
                    }
                    Label { Layout.fillWidth: true; text: qsTr("Game fonts are shared across profiles and stay installed when switching profiles."); color: Theme.secondary; wrapMode: Text.Wrap; font.pointSize: Theme.small }
                    Flow {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        Button { text: qsTr("Manage game fonts…"); enabled: root.library.fonts.available && !root.library.fonts.busy; onClicked: root.library.fonts.showInstalled() }
                        Button { text: qsTr("Undo last font change…"); icon.source: Theme.icon("undo"); enabled: root.library.fonts.canUndo; onClicked: root.library.fonts.previewUndo() }
                    }
                }
            }

            Section {
                title: qsTr("About")
                Label {
                    Layout.fillWidth: true
                    objectName: "settingsBuildId"
                    text: qsTr("Modlock · Build %1").arg(buildIdentifier || qsTr("unknown"))
                    textFormat: Text.PlainText
                    color: Theme.secondary
                }
                Label {
                    Layout.fillWidth: true
                    text: qsTr("A native Linux manager for Deadlock mods. Mods come from GameBanana and their authors; please support them.")
                    wrapMode: Text.Wrap
                    color: Theme.secondary
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Theme.spacing8
                    Button { text: qsTr("GameBanana"); flat: true; icon.source: Theme.icon("external"); onClicked: Qt.openUrlExternally("https://gamebanana.com/games/20948") }

                }
            }
            Item { Layout.preferredHeight: Theme.spacing24 }
        }
    }
}
