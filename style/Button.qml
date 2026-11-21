import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.Button {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding)
    padding: 6
    leftPadding: Tokens.controlPadding
    rightPadding: Tokens.controlPadding
    spacing: 6
    icon.width: 16
    icon.height: 16
    icon.color: control.highlighted ? Tokens.accentText : (control.enabled ? Tokens.text : Tokens.textDisabled)
    font.weight: control.highlighted ? Font.DemiBold : Font.Medium

    contentItem: IconLabel {
        spacing: control.spacing
        mirrored: control.mirrored
        display: control.display
        icon: control.icon
        text: control.text
        font: control.font
        color: !control.enabled ? Tokens.textDisabled
             : control.highlighted ? Tokens.accentText
             : control.checked ? Tokens.accentInk : Tokens.text
    }

    background: Rectangle {
        implicitWidth: 64
        implicitHeight: Tokens.controlHeight
        radius: Tokens.radius
        color: {
            if (control.highlighted) {
                if (!control.enabled) return Tokens.surface3
                return control.down ? Tokens.accentPressed : control.hovered ? Tokens.accentHover : Tokens.accent
            }
            if (control.checked) return control.hovered ? Qt.alpha(Tokens.accent, 0.28) : Tokens.accentSoft
            if (control.flat) return control.down ? Tokens.surface3 : control.hovered ? Tokens.surface2 : "transparent"
            return control.down ? Tokens.surface4 : control.hovered ? Tokens.surface3 : Tokens.surface2
        }
        border.width: control.flat || control.highlighted || control.checked ? 0 : 1
        border.color: Tokens.borderStrong
        Rectangle {
            anchors.fill: parent
            anchors.margins: -4
            radius: parent.radius + 4
            color: "transparent"
            border.width: 2
            border.color: Tokens.focus
            visible: control.visualFocus
        }
        Behavior on color { ColorAnimation { duration: Tokens.fast } }
    }
}
