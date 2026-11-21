import QtQuick
import QtQuick.Controls
import Deadlock.Native

Switch {
    id: control
    implicitWidth: Math.max(44 + leftPadding + rightPadding, implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(24 + topPadding + bottomPadding, implicitContentHeight + topPadding + bottomPadding)
    contentItem: Text {
        text: control.text
        textFormat: Text.PlainText
        font: control.font
        color: control.palette.text
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: control.mirrored ? Text.AlignRight : Text.AlignLeft
        leftPadding: control.text && !control.mirrored ? 44 + control.spacing : 0
        rightPadding: control.text && control.mirrored ? 44 + control.spacing : 0
    }
    indicator: Rectangle {
        implicitWidth: 44
        implicitHeight: 24
        x: control.text ? (control.mirrored ? control.width - control.rightPadding - width : control.leftPadding) : (control.width - width) / 2
        y: (control.height - height) / 2
        radius: height / 2
        color: control.checked ? Theme.accent : Theme.surface
        border.width: 1
        border.color: control.checked ? Theme.accent : Theme.text
        opacity: control.enabled ? 1 : 0.55
        Rectangle {
            anchors.fill: parent
            anchors.margins: -3
            color: "transparent"
            radius: height / 2
            border.width: 2
            border.color: Theme.accent
            visible: control.visualFocus
        }
        Rectangle {
            width: 16
            height: 16
            radius: 8
            y: (parent.height - height) / 2
            x: control.checked ? parent.width - width - 4 : 4
            color: control.checked ? Theme.selectionText : Theme.text
        }
    }
}
