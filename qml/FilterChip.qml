import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import Deadlock.Native

T.Button {
    id: control
    property int count: -1
    Accessible.name: count >= 0 ? qsTr("%1, %2").arg(text).arg(count) : text
    checkable: true
    focusPolicy: Qt.StrongFocus
    padding: 12
    topPadding: 4
    bottomPadding: 4
    spacing: 6
    implicitWidth: implicitContentWidth + leftPadding + rightPadding
    implicitHeight: Math.max(28, implicitContentHeight + topPadding + bottomPadding)
    icon.width: 16
    icon.height: 16
    icon.color: checked ? Theme.accentInk : Theme.secondary
    icon.source: checked ? Theme.icon("check") : ""
    contentItem: Row {
        spacing: control.spacing
        IconLabel {
            icon: control.icon
            text: control.text
            font: control.font
            color: control.checked ? Theme.text : Theme.secondary
        }
        Text {
            visible: control.count >= 0
            text: control.count
            color: Theme.warning
            font.family: control.font.family
            font.pointSize: control.font.pointSize
            font.weight: Font.DemiBold
        }
    }
    background: Rectangle {
        radius: 14
        color: control.checked ? Theme.accentSoft : "transparent"
        border.width: control.visualFocus ? 2 : control.checked ? 0 : 1
        border.color: control.visualFocus ? Theme.focus : Theme.ruleStrong
    }
}
