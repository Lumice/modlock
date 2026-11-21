pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

Rectangle {
    id: card
    property string label
    property string value
    property string iconName
    property alias extra: extraSlot.data
    Layout.fillWidth: true
    Layout.preferredWidth: 160
    implicitHeight: cardRow.implicitHeight + Theme.spacing24
    radius: Theme.radius
    color: Theme.surfaceLow
    border.color: Theme.rule
    RowLayout {
        id: cardRow
        anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: Theme.spacing12; rightMargin: Theme.spacing8 }
        spacing: Theme.spacing12
        ColorImage {
            visible: card.iconName !== ""
            source: card.iconName ? Theme.icon(card.iconName) : ""
            sourceSize: Qt.size(18, 18)
            Layout.preferredWidth: 18
            Layout.preferredHeight: 18
            color: Theme.secondary
            Accessible.ignored: true
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label { text: card.label; color: Theme.secondary; font.pointSize: Theme.small; textFormat: Text.PlainText }
            Label { Layout.fillWidth: true; text: card.value; font.weight: Font.DemiBold; textFormat: Text.PlainText; elide: Text.ElideRight }
        }
        Item { id: extraSlot; implicitWidth: childrenRect.width; implicitHeight: childrenRect.height }
    }
}
