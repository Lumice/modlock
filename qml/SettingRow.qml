import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic
import Deadlock.Native

RowLayout {
    id: root
    property string title: ""
    property string description: ""
    default property alias control: trailing.data
    spacing: 16
    ColumnLayout {
        Layout.fillWidth: true
        spacing: 4
        Label { text: root.title; textFormat: Text.PlainText; font.weight: Font.DemiBold; color: Theme.text; wrapMode: Text.Wrap; Layout.fillWidth: true }
        Label { visible: root.description.length > 0; text: root.description; textFormat: Text.PlainText; color: Theme.secondary; font.pointSize: Theme.small; wrapMode: Text.Wrap; Layout.fillWidth: true }
    }
    RowLayout { id: trailing; Layout.alignment: Qt.AlignRight | Qt.AlignVCenter }
}
