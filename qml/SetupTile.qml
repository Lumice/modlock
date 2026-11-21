import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

AbstractButton {
    id: root
    property string iconName
    property string title
    property string value
    Layout.fillWidth: true
    Layout.preferredWidth: 1
    implicitHeight: Math.max(94, implicitContentHeight + topPadding + bottomPadding)
    padding: 16
    focusPolicy: Qt.StrongFocus
    Accessible.name: title + (value ? ": " + value : "")
    ToolTip.visible: hovered || activeFocus
    ToolTip.text: Accessible.name
    background: Rectangle {
        color: root.down ? Theme.surfacePressed : root.hovered ? Theme.surfaceHover : Theme.surface
        radius: Theme.radiusCard
        border.color: Theme.rule
        Rectangle { anchors.fill: parent; anchors.margins: -3; radius: Theme.radiusCard + 3; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: root.visualFocus }
    }
    contentItem: ColumnLayout {
        spacing: 8
        RowLayout {
            Rectangle {
                Layout.preferredWidth: 28; Layout.preferredHeight: 28
                radius: 14; color: Theme.brassSoft
                ColorImage { anchors.centerIn: parent; source: Theme.icon(root.iconName); sourceSize: Qt.size(20, 20); color: Theme.brass }
            }
            Label { text: root.title; color: Theme.secondary; font.pointSize: Theme.small; Layout.fillWidth: true; elide: Text.ElideRight }
        }
        Label { text: root.value; textFormat: Text.PlainText; Layout.fillWidth: true; elide: Text.ElideRight; font.weight: Font.DemiBold }
    }
}
