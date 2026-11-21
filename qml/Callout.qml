import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import Deadlock.Native

Rectangle {
    id: root
    property string tone: "info"
    property string title: ""
    property string text: ""
    default property alias actions: actionsRow.data
    readonly property color ink: tone === "warning" ? Theme.warning : tone === "danger" ? Theme.danger : tone === "success" ? Theme.success : Theme.accentInk
    implicitWidth: 360
    implicitHeight: body.implicitHeight + 32
    radius: Theme.radiusCard
    color: tone === "warning" ? Theme.warningSoft : tone === "danger" ? Theme.dangerSoft : tone === "success" ? Theme.successSoft : Theme.accentSoft
    border.color: Qt.alpha(ink, .28)
    RowLayout {
        id: body
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12
        ColorImage {
            Layout.alignment: Qt.AlignTop
            Layout.preferredWidth: 20
            Layout.preferredHeight: 20
            sourceSize: Qt.size(20, 20)
            source: Theme.icon(root.tone === "success" ? "check" : root.tone === "info" ? "info" : "alert")
            color: root.ink
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 8
            Label { visible: root.title.length > 0; text: root.title; textFormat: Text.PlainText; color: Theme.text; font.weight: Font.DemiBold; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: root.text.length > 0; text: root.text; textFormat: Text.PlainText; color: Theme.secondary; wrapMode: Text.Wrap; Layout.fillWidth: true }
            RowLayout { id: actionsRow; visible: children.length > 0 }
        }
    }
}
