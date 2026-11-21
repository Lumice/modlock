pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Deadlock.Native

Item {
    id: root
    required property LibraryModel library
    property bool crashReviewRequested: false
    readonly property bool strongCrash: library.launch.health.report.classification === "crash" || !!library.launch.health.report.confirmed
    property bool detailsOpen: false
    property bool settingsOpen: false
    property bool settingsCreated: false
    property bool reviewWasVisible: false
    property bool closing: false
    property string windowCloseStamp: ""
    readonly property bool settingsDirty: settingsLoader.status === Loader.Ready && settingsLoader.item && settingsLoader.item.dirty
    readonly property bool popupOpen: (Object.keys(root.library.launch.health.report).length > 0 && (strongCrash || crashReviewRequested)) || settingsOpen || detailsOpen || reviewLoader.active || crashConfigReview.active || windowCloseConfirmation.visible
        || (settingsLoader.status === Loader.Ready && settingsLoader.item && settingsLoader.item.popupOpen)
    property var crashConfigController: null
    Connections {
        target: root.library.launch.health
        function onChanged() {
            if (Object.keys(root.library.launch.health.report).length === 0) root.crashReviewRequested = false
        }
        function onConfigReviewRequested(kind) {
            root.crashConfigController = kind === "autoexec" ? root.library.autoexec : root.library.crosshair
            crashConfigReview.active = true
        }
    }
    Loader {
        id: crashConfigReview
        active: false
        sourceComponent: ConfigReviewDialog {
            controller: root.crashConfigController
            heading: qsTr("Restore previous settings")
            Component.onCompleted: open()
            onClosed: crashConfigReview.active = false
        }
    }
    signal findBrokenMod()
    signal reviewDismissed()
    signal detailsDismissed()
    signal settingsDismissed()
    signal windowCloseReady()

    function requestWindowClose() {
        if (!settingsDirty) return true
        if (!windowCloseConfirmation.visible) {
            windowCloseStamp = settingsLoader.item.draftStamp
            windowCloseConfirmation.open()
        }
        return false
    }
    onSettingsOpenChanged: {
        if (!settingsOpen) return
        if (closing) { settingsOpen = false; return }
        settingsCreated = true
        if (settingsLoader.status === Loader.Ready && settingsLoader.item) settingsLoader.item.openEditor()
    }
    Loader {
        active: Object.keys(root.library.launch.health.report).length > 0 && (root.strongCrash || root.crashReviewRequested)
        sourceComponent: CrashDialog { library: root.library; onFindBrokenMod: root.findBrokenMod() }
    }
    Popup {
        id: crashNotice
        objectName: "crashNotice"
        parent: Overlay.overlay
        x: Math.max(0, (parent.width - width) / 2)
        y: 16
        width: Math.min(440, parent.width - 32)
        modal: false
        closePolicy: Popup.NoAutoClose
        visible: Object.keys(root.library.launch.health.report).length > 0 && !root.strongCrash && !root.crashReviewRequested
        contentItem: Row {
            spacing: 12
            Label { text: qsTr("Deadlock closed soon after your setup changed."); wrapMode: Text.Wrap; width: Math.max(100, crashNotice.availableWidth - reviewCrash.width - dismissCrash.width - 24) }
            Button { id: reviewCrash; objectName: "reviewCrashNoticeButton"; text: qsTr("Review"); onClicked: root.crashReviewRequested = true }
            Button { id: dismissCrash; text: qsTr("Dismiss"); onClicked: root.library.launch.health.dismiss() }
        }
    }
    Loader {
        id: reviewLoader
        active: root.library.launch.canCommit && root.library.launch.review.changed === true
        sourceComponent: LaunchReview { library: root.library }
        onLoaded: root.reviewWasVisible = true
        onActiveChanged: if (!active && root.reviewWasVisible) { root.reviewWasVisible = false; root.reviewDismissed() }
    }
    Loader {
        active: root.detailsOpen
        sourceComponent: LaunchDetails {
            library: root.library
            onDismissed: { root.detailsOpen = false; root.detailsDismissed() }
        }
    }
    Loader {
        id: settingsLoader
        active: root.settingsCreated
        sourceComponent: LaunchSettings {
            library: root.library
            closing: root.closing
            windowClosePrompt: windowCloseConfirmation.visible
            onDismissed: { root.settingsOpen = false; root.settingsDismissed() }
        }
        onLoaded: if (root.settingsOpen && !root.closing) item.openEditor()
    }
    Dialog {
        id: windowCloseConfirmation
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(440, parent ? parent.width - 32 : 440)
        modal: true
        Overlay.modal: Rectangle { color: Theme.overlay }
        title: qsTr("Discard unsaved launch arguments?")
        standardButtons: Dialog.Ok | Dialog.Cancel
        Component.onCompleted: standardButton(Dialog.Ok).text = qsTr("Discard")
        onOpened: standardButton(Dialog.Cancel).forceActiveFocus()
        contentItem: Label {
            text: qsTr("Discard this draft and close? A save already started may still finish.")
            wrapMode: Text.Wrap
        }
        onAccepted: {
            if (!root.settingsDirty) { root.windowCloseReady(); return }
            if (root.windowCloseStamp !== settingsLoader.item.draftStamp) {
                Qt.callLater(() => root.requestWindowClose())
                return
            }
            settingsLoader.item.discardDraft()
            root.windowCloseReady()
        }
    }
}
