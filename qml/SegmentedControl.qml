pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import Deadlock.Native

FocusScope {
    id: root
    property var model: []
    property var currentValue
    signal activated(var value)
    implicitWidth: segments.implicitWidth
    implicitHeight: segments.implicitHeight
    function move(index, delta) {
        const next = (index + delta + root.model.length) % root.model.length
        root.activated(root.model[next].value)
        choices.itemAt(next).forceActiveFocus(Qt.TabFocusReason)
    }
    Rectangle {
        anchors.fill: parent
        radius: Theme.radius
        color: "transparent"
        border.color: Theme.ruleStrong
    }
    RowLayout {
        id: segments
        anchors.fill: parent
        spacing: 0
        Repeater {
            id: choices
            model: root.model
            Button {
                id: segment
                required property var modelData
                required property int index
                Layout.fillWidth: true
                objectName: modelData.objectName || ""
                text: modelData.text
                icon.source: modelData.icon ? Theme.icon(modelData.icon) : ""
                icon.color: checked ? Theme.accentInk : Theme.secondary
                // The consumer owns selection, so clicking only requests a change.
                checkable: false
                checked: root.currentValue === modelData.value
                Accessible.role: Accessible.RadioButton
                Accessible.name: text
                Accessible.checkable: true
                Accessible.checked: checked
                onClicked: root.activated(modelData.value)
                Keys.onLeftPressed: root.move(index, -1)
                Keys.onRightPressed: root.move(index, 1)
                contentItem: IconLabel {
                    icon: segment.icon
                    text: segment.text
                    font: segment.font
                    spacing: 6
                    color: segment.checked ? Theme.accentInk : Theme.secondary
                }
                background: Rectangle {
                    radius: Theme.radius
                    color: segment.checked ? Theme.accentSoft : "transparent"
                    border.width: segment.visualFocus ? 2 : 0
                    border.color: Theme.focus
                }
            }
        }
    }
}
