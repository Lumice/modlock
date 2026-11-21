import QtQuick
import QtQuick.Controls
import Deadlock.Native
import QtQuick.Controls.impl

// Small rounded label for a state such as "Applied" or "Not applied yet".
// tone: "" (neutral), "success", "warning", "danger", "accent", "brass" or "muted".
Rectangle {
    id: root
    property alias text: label.text
    property alias textFormat: label.textFormat
    property string iconName: ""
    property string tone: ""
    readonly property color ink: tone === "success" ? Theme.success : tone === "warning" ? Theme.warning
                               : tone === "danger" ? Theme.danger : tone === "accent" ? Theme.accentInk
                               : tone === "brass" ? Theme.brass
                               : tone === "muted" ? Theme.faint : Theme.secondary
    implicitWidth: label.implicitWidth
    implicitHeight: Math.max(22, label.implicitHeight + 4)
    radius: height / 2
    color: tone === "success" ? Theme.successSoft : tone === "warning" ? Theme.warningSoft
         : tone === "danger" ? Theme.dangerSoft : tone === "accent" ? Theme.accentSoft : tone === "brass" ? Theme.brassSoft : Theme.surfaceHigh
    Accessible.role: Accessible.StaticText
    Accessible.name: label.text
    ColorImage {
        anchors.left: parent.left
        anchors.leftMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 16; height: 16
        sourceSize: Qt.size(16, 16)
        visible: root.iconName.length > 0
        source: root.iconName ? Theme.icon(root.iconName) : ""
        color: root.ink
    }
    Label {
        id: label
        textFormat: Text.PlainText
        anchors.fill: parent
        leftPadding: root.iconName ? 28 : 8
        rightPadding: 8
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
        font.pointSize: Theme.small
        font.weight: Font.Medium
        color: root.ink
    }
}
