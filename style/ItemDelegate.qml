import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.ItemDelegate {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    padding: 8
    leftPadding: 12
    rightPadding: 12
    spacing: 8
    icon.width: 16
    icon.height: 16
    icon.color: Tokens.text
    contentItem: IconLabel {
        spacing: control.spacing
        mirrored: control.mirrored
        display: control.display
        alignment: control.display === IconLabel.IconOnly || control.display === IconLabel.TextUnderIcon ? Qt.AlignCenter : Qt.AlignLeft
        icon: control.icon
        text: control.text
        font: control.font
        color: control.enabled ? Tokens.text : Tokens.textDisabled
    }
    background: Rectangle {
        implicitWidth: 100
        implicitHeight: 32
        radius: Tokens.radiusSmall
        color: control.down ? Tokens.surface4 : control.highlighted ? Tokens.accentSoft : control.hovered ? Tokens.surface3 : "transparent"
        border.width: control.visualFocus ? 2 : 0
        border.color: Tokens.focus
    }
}
