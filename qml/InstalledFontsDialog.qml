pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "FontPresentation.js" as FontPresentation

Dialog {
    id: root
    required property FontController controller
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(820, parent ? parent.width - 32 : 820)
    height: Math.min(690, parent ? parent.height - 32 : 690)
    title: qsTr("Game fonts")
    modal: false
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: { open(); closeButton.forceActiveFocus() }
    onClosed: controller.closeInstalled()
    ColumnLayout {
        anchors.fill: parent
        Label {
            Layout.fillWidth: true
            text: qsTr("Game: %1\nGlobal installed owners remain here after their source mod or profile is removed. Choose a source mod to review removing its fonts. Shared and borrowed files follow the reviewed ownership rules.").arg(root.controller.gamePath)
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        Label {
            Layout.fillWidth: true
            visible: root.controller.installedOwners.length === 0
            text: qsTr("No installed font owners in this library.")
            textFormat: Text.PlainText
        }
        Frame {
            Layout.fillWidth: true
            Layout.fillHeight: true
            padding: 2
            background: Rectangle {
                color: Theme.surface
                border.color: owners.activeFocus ? Theme.accent : Theme.rule
                border.width: owners.activeFocus ? 2 : 1
            }
            ListView {
                id: owners
                anchors.fill: parent
                activeFocusOnTab: true
                keyNavigationEnabled: false
                Accessible.name: qsTr("Installed font owners")
                Accessible.description: qsTr("Use Up and Down to scroll by pixels, Page Up and Page Down to scroll by pages, and Tab to reach removal controls.")
                Keys.onPressed: event => FontPresentation.scrollKey(owners, event, Math.max(24, Theme.body * 2))
                clip: true
                reuseItems: true
                spacing: 12
                model: root.controller.installedOwners
                ScrollBar.vertical: ScrollBar {}
                delegate: ColumnLayout {
                    id: owner
                    required property var modelData
                    width: owners.width
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Target: %1\nFamily: %2\nSource mod: %3\nAsset: %4\nOwners sharing this file: %5")
                            .arg(owner.modelData.targetName).arg(owner.modelData.displayName)
                            .arg(owner.modelData.modId).arg(owner.modelData.fontId)
                            .arg(Number(owner.modelData.ownerCount).toLocaleString(Qt.locale(), 'f', 0))
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                    Button {
                        text: qsTr("Review removal for this source mod…")
                        enabled: root.controller.available && !root.controller.busy
                        onClicked: root.controller.open(owner.modelData.modId, "remove")
                    }
                }
            }
        }
        Button {
            id: closeButton
            text: qsTr("Close")
            KeyNavigation.backtab: owners
            KeyNavigation.tab: owners
            onClicked: root.close()
        }
    }
}
