import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import QtQuick.Templates as T
import Deadlock.Native

T.AbstractButton {
    id: control
    property string label: ""
    property string summary: ""
    checkable: true
    focusPolicy: Qt.StrongFocus
    ToolTip.visible: summary !== "" && (hovered || activeFocus)
    ToolTip.text: summary
    ToolTip.delay: 500
    Accessible.description: summary
    Accessible.name: label
    Accessible.role: Accessible.CheckBox
    implicitWidth: implicitContentWidth + 16
    implicitHeight: Math.max(34, implicitContentHeight + 8)
    padding: 8
    contentItem: RowLayout {
        spacing: 8
        ColorImage {
            source: Theme.icon("chevron-right")
            sourceSize: Qt.size(16, 16)
            Layout.preferredWidth: 16
            Layout.preferredHeight: 16
            color: Theme.secondary
            rotation: control.checked ? 90 : 0
            Behavior on rotation { NumberAnimation { duration: Theme.motionFast } }
        }
        Label { text: control.label; color: Theme.text; textFormat: Text.PlainText }
        Label { text: control.summary; color: Theme.faint; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
    }
    background: Rectangle {
        color: control.hovered ? Theme.surfaceHover : "transparent"
        radius: Theme.radius
        border.width: control.visualFocus ? 2 : 0
        border.color: Theme.focus
    }
}
