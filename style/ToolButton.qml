import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.ToolButton {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding)
    padding: 6
    spacing: 6
    icon.width: 18
    icon.height: 18
    icon.color: control.checked ? Tokens.accentInk : control.enabled ? Tokens.text : Tokens.textDisabled
    flat: true
    contentItem: IconLabel {
        spacing: control.spacing
        mirrored: control.mirrored
        display: control.display
        icon: control.icon
        text: control.text
        font: control.font
        color: control.checked ? Tokens.accentInk : control.enabled ? Tokens.text : Tokens.textDisabled
    }
    background: Rectangle {
        implicitWidth: Tokens.controlHeight
        implicitHeight: Tokens.controlHeight
        radius: Tokens.radiusSmall
        color: control.down ? Tokens.surface4 : control.checked ? Tokens.accentSoft : control.hovered ? Tokens.surface3 : "transparent"
        border.width: control.visualFocus ? 2 : 0
        border.color: Tokens.focus
    }
}
