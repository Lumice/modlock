import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.Switch {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    padding: 4
    spacing: 10
    indicator: Rectangle {
        implicitWidth: 40
        implicitHeight: 22
        x: control.text ? (control.mirrored ? control.width - width - control.rightPadding : control.leftPadding) : control.leftPadding + (control.availableWidth - width) / 2
        y: control.topPadding + (control.availableHeight - height) / 2
        radius: height / 2
        color: !control.enabled ? Tokens.surface3 : control.checked ? Tokens.accent : Tokens.surface4
        border.width: control.checked ? 0 : 1
        border.color: control.enabled ? Tokens.fieldBorder : Tokens.borderStrong
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
        Rectangle {
            width: 16
            height: 16
            radius: 8
            y: (parent.height - height) / 2
            x: control.checked ? parent.width - width - 3 : 3
            color: !control.enabled ? Tokens.textDisabled : control.checked ? Tokens.accentText : Tokens.text
            Behavior on x { enabled: !control.down; NumberAnimation { duration: Tokens.normal; easing.type: Easing.OutCubic } }
        }
    }
    contentItem: Text {
        leftPadding: control.indicator && !control.mirrored ? control.indicator.width + control.spacing : 0
        rightPadding: control.indicator && control.mirrored ? control.indicator.width + control.spacing : 0
        text: control.text
        font: control.font
        color: control.enabled ? Tokens.text : Tokens.textDisabled
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
    }
}
