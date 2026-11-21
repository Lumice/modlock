import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.TabButton {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding)
    padding: 8
    leftPadding: 14
    rightPadding: 14
    spacing: 6
    icon.width: 16
    icon.height: 16
    icon.color: control.checked ? Tokens.text : Tokens.textMuted
    font.weight: control.checked ? Font.DemiBold : Font.Medium
    contentItem: IconLabel {
        spacing: control.spacing
        mirrored: control.mirrored
        display: control.display
        icon: control.icon
        text: control.text
        font: control.font
        color: !control.enabled ? Tokens.textDisabled : control.checked ? Tokens.text : control.hovered ? Tokens.text : Tokens.textMuted
    }
    background: Rectangle {
        implicitHeight: Tokens.controlHeight
        color: control.down ? Tokens.surface3 : control.hovered && !control.checked ? Tokens.surface2 : "transparent"
        radius: Tokens.radiusSmall
        border.width: control.visualFocus ? 2 : 0
        border.color: Tokens.focus
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            height: 2
            radius: 1
            color: Tokens.accent
            visible: control.checked
        }
    }
}
