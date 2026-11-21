pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
Dialog {
    id: root
    required property OptionController controller
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(760, parent ? parent.width - 32 : 760)
    height: Math.min(680, parent ? parent.height - 32 : 680)
    title: qsTr("Load order")
    property bool autoSave: false
    Connections {
        target: root.controller
        function onChanged() {
            if (!root.autoSave) return
            if (root.controller.error) { root.autoSave = false; return }
            if (root.controller.canSave) { root.autoSave = false; root.controller.commitSelection(); return }
            // A finished review that cannot be committed (e.g. nothing changed) ends this Save.
            if (root.controller.selectionReview.token && !root.controller.busy) root.autoSave = false
        }
    }
    modal: false
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: { open(); orderList.forceActiveFocus() }
    onClosed: controller.hide(true)
    ColumnLayout {
        anchors.fill: parent
        Label { Layout.fillWidth: true; text: root.controller.profileName; textFormat: Text.PlainText; elide: Text.ElideRight; font.weight: Font.Medium }
        Label {
            Layout.fillWidth: true
            text: qsTr("When two mods change the same file, the one nearer the top wins. Apply changes warns about real conflicts.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Select a mod, then use the arrows or Alt+Up / Alt+Down.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.pointSize: Theme.small
            color: Theme.faint
        }
        ListView {
            id: orderList
            visible: !root.controller.selectionReview.token
            Layout.fillWidth: true
            Layout.fillHeight: true
            model: root.controller.order
            reuseItems: true
            clip: true
            activeFocusOnTab: true
            ScrollBar.vertical: ScrollBar {}
            Accessible.name: qsTr("Full profile install order")
            function moveCurrent(offset) {
                if (currentIndex < 0 || currentIndex >= count) return
                const position = currentIndex
                const id = root.controller.order[position].id
                if (root.controller.move(id, offset)) currentIndex = Math.max(0, Math.min(count - 1, position + offset))
            }
            Keys.onPressed: event => {
                if ((event.modifiers & Qt.AltModifier) && (event.key === Qt.Key_Up || event.key === Qt.Key_Down)) {
                    orderList.moveCurrent(event.key === Qt.Key_Up ? -1 : 1)
                    event.accepted = true
                }
            }
            spacing: 4
            delegate: ItemDelegate {
                id: orderRow
                required property var modelData
                required property int index
                width: orderList.width
                highlighted: orderList.currentIndex === index
                onClicked: orderList.currentIndex = index
                Accessible.name: qsTr("%1. %2").arg(index + 1).arg(modelData.title)
                contentItem: RowLayout {
                    spacing: Theme.spacing12
                    Rectangle {
                        Layout.preferredWidth: 28; Layout.preferredHeight: 28; radius: 14
                        color: orderRow.highlighted ? Theme.accent : Theme.surfaceHigh
                        Label { anchors.centerIn: parent; text: orderRow.index + 1; font.weight: Font.DemiBold; color: orderRow.highlighted ? Theme.selectionText : Theme.secondary }
                    }
                    Label {
                        Layout.fillWidth: true
                        text: orderRow.modelData.title
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        font.weight: Font.Medium
                        color: orderRow.modelData.enabled ? Theme.text : Theme.faint
                    }
                    StatusPill { visible: !orderRow.modelData.enabled; text: qsTr("Off"); tone: "muted" }
                    StatusPill { visible: !orderRow.modelData.available; text: qsTr("Unavailable"); tone: "danger" }
                    ToolButton {
                        visible: orderRow.highlighted
                        icon.source: Theme.icon("chevron-left"); rotation: 90
                        display: AbstractButton.IconOnly; text: qsTr("Move up")
                        enabled: root.controller.editable && orderRow.index > 0
                        onClicked: orderList.moveCurrent(-1)
                    }
                    ToolButton {
                        visible: orderRow.highlighted
                        icon.source: Theme.icon("chevron-down")
                        display: AbstractButton.IconOnly; text: qsTr("Move down")
                        enabled: root.controller.editable && orderRow.index + 1 < orderList.count
                        onClicked: orderList.moveCurrent(1)
                    }
                }
                background: Rectangle {
                    radius: Theme.radius
                    color: orderRow.highlighted ? Theme.accentSoft : orderRow.hovered ? Theme.surface : Theme.surfaceLow
                    border.color: orderRow.highlighted ? Theme.accent : Theme.rule
                }
            }
        }
        RowLayout {
            Layout.fillWidth: true
            visible: false
            Button { text: qsTr("Move up"); enabled: root.controller.editable && orderList.currentIndex > 0; onClicked: orderList.moveCurrent(-1) }
            Button { text: qsTr("Move down"); enabled: root.controller.editable && orderList.currentIndex >= 0 && orderList.currentIndex + 1 < orderList.count; onClicked: orderList.moveCurrent(1) }
        }
        ProfileEditReview { Layout.fillWidth: true; Layout.fillHeight: true; review: root.controller.selectionReview }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(80, errorText.implicitHeight)
            visible: !!root.controller.error
            contentWidth: availableWidth
            Label { id: errorText; width: parent.width; text: root.controller.error; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere }
        }
        RowLayout {
            Layout.fillWidth: true
            Button { text: qsTr("Close"); onClicked: root.close() }
            Button { text: qsTr("Undo edits"); flat: true; enabled: !root.controller.busy; onClicked: { root.autoSave = false; root.controller.cancel() } }
            Item { Layout.fillWidth: true }
            Button { flat: true; text: root.controller.selectionReview.token ? qsTr("Back to order") : qsTr("Show changes"); enabled: root.controller.editable; onClicked: { root.autoSave = false; root.controller.selectionReview.token ? root.controller.discardSelectionReview() : root.controller.previewOrder() } }
            Button {
                highlighted: true
                text: qsTr("Save")
                enabled: root.controller.canSave || (root.controller.editable && !root.controller.selectionReview.token && !root.controller.busy)
                onClicked: {
                    if (root.controller.canSave) root.controller.commitSelection()
                    else { root.autoSave = true; if (!root.controller.previewOrder()) root.autoSave = false }
                }
            }
        }
    }
}
