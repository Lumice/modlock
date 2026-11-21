pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Deadlock.Native

Dialog {
    id: root
    required property LibraryModel library
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(650, parent ? parent.width - 32 : 650)
    height: Math.min(590, parent ? parent.height - 32 : 590)
    title: qsTr("Update setup and play")
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: { open(); cancelButton.forceActiveFocus() }
    onRejected: if (library.launch.canCancel) library.launch.cancel()
    contentItem: LaunchSummary {
        controller: root.library.launch
        profiles: root.library.profiles
        installedFonts: root.library.fonts.installedOwners.length > 0
    }
    footer: DialogButtonBox {
        Button {
            id: cancelButton
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            objectName: "launchCommitButton"
            text: qsTr("Update setup and play")
            enabled: root.library.launch.canCommit
            DialogButtonBox.buttonRole: DialogButtonBox.ActionRole
            onClicked: root.library.launch.commit()
        }
        onRejected: root.reject()
    }
}
