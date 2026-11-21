pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Deadlock.Native
Dialog {
    id: root
    required property LibraryModel library
    required property StorageController controller
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(680, parent ? parent.width - 32 : 680)
    modal: true
    title: qsTr("Move library storage")
    footer: Control {
        padding: Theme.spacing16
        contentItem: RowLayout {
            spacing: Theme.spacing8
            Item { Layout.fillWidth: true }
            Button {
                objectName: "storageMoveConfirm"
                text: qsTr("Move storage")
                highlighted: true
                enabled: !!root.controller.result.token && !root.controller.busy
                onClicked: root.controller.move()
            }
            Button {
                text: qsTr("Close")
                enabled: !root.controller.busy
                onClicked: root.close()
            }
        }
    }
    closePolicy: controller.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    Overlay.modal: Rectangle { color: Theme.overlay }
    function space(value) { return qsTr("%1 GB").arg((Number(value || 0) / 1073741824).toFixed(2)) }
    function interruptionText(canCancel) {
        return canCancel
            ? qsTr("A storage move was interrupted. Resume starts a fresh copy in a new folder. Cancel keeps the source active. Partial copies may contain files you changed after the move stopped — check it before deleting.")
            : qsTr("The storage location has switched. Resume finishes removing verified old copies. Cancel is no longer available. Keep both drives connected until finished.")
    }
    Component.onCompleted: open()
    FolderDialog {
        id: picker
        title: qsTr("Choose an empty storage folder")
        onAccepted: root.controller.review(decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label { Layout.fillWidth: true; text: qsTr("Move mod files, saved archives and download preparation to another drive. Your library database and settings stay in the current library folder."); wrapMode: Text.Wrap }
        Frame {
            Layout.fillWidth: true
            ColumnLayout {
                anchors.fill: parent
                Label { text: qsTr("From"); font.bold: true }
                Label {
                    Layout.fillWidth: true
                    text: root.controller.result.source?.path || root.controller.result.current?.path || root.library.dataDir
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Label {
                    text: qsTr("%1 used · %2 free").arg(root.space(root.controller.result.source?.driveUsedBytes ?? root.controller.result.current?.driveUsedBytes)).arg(root.space(root.controller.result.source?.freeBytes ?? root.controller.result.current?.freeBytes))
                    color: Theme.secondary
                }
            }
        }
        Frame {
            Layout.fillWidth: true
            ColumnLayout {
                anchors.fill: parent
                RowLayout {
                    Layout.fillWidth: true
                    Label { Layout.fillWidth: true; text: qsTr("To"); font.bold: true }
                    Button {
                        objectName: "storageChooseFolder"
                        text: qsTr("Choose folder…")
                        enabled: !root.controller.busy && !root.controller.result.resume
                        onClicked: picker.open()
                    }
                }
                Label {
                    Layout.fillWidth: true
                    text: root.controller.result.destination?.path || qsTr("Choose an empty folder on the drive you want to use.")
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Label {
                    visible: !!root.controller.result.token
                    text: qsTr("%1 used · %2 free").arg(root.space(root.controller.result.destination?.driveUsedBytes)).arg(root.space(root.controller.result.destination?.freeBytes))
                    color: Theme.secondary
                }
            }
        }
        Label { visible: !!root.controller.result.token; text: qsTr("%1 to move").arg(root.space(root.controller.result.bytesToMove)) }
        Label { Layout.fillWidth: true; visible: !!root.controller.result.token; text: qsTr("Files are copied and checked before switching. The old copies are then removed. Keep both drives connected until finished."); wrapMode: Text.Wrap; color: Theme.secondary }
        Label { objectName: "storageInterruptedBanner"; Layout.fillWidth: true; visible: !!root.controller.result.resume; text: root.interruptionText(!!root.controller.result.canCancel); wrapMode: Text.Wrap; color: Theme.warning }
        RowLayout {
        Button { objectName: "storageResume"; visible: !!root.controller.result.resume; text: qsTr("Resume move"); enabled: !root.controller.busy; onClicked: root.controller.resume() }
        Button { objectName: "storageCancel"; visible: !!root.controller.result.canCancel; text: qsTr("Cancel move"); enabled: !root.controller.busy; onClicked: root.controller.cancel() }
        }
        Label { Layout.fillWidth: true; visible: !!root.controller.notice; text: root.controller.notice; textFormat: Text.PlainText; wrapMode: Text.Wrap }
        ProgressBar { Layout.fillWidth: true; visible: root.controller.busy; indeterminate: root.controller.progress < 0; value: Math.max(0, root.controller.progress) }
        Label { visible: root.controller.busy; text: root.controller.phase }
        Label { Layout.fillWidth: true; visible: !!root.controller.error; text: Theme.friendlyError(root.controller.error); textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.warning }
    }
}
