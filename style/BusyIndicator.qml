import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.BusyIndicator {
    id: control
    implicitWidth: implicitContentWidth + leftPadding + rightPadding
    implicitHeight: implicitContentHeight + topPadding + bottomPadding
    padding: 2
    contentItem: Item {
        implicitWidth: 24
        implicitHeight: 24
        opacity: control.running ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: Tokens.normal } }
        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: "transparent"
            border.width: Math.max(2, width / 10)
            border.color: Tokens.surface4
        }
        Item {
            id: arc
            anchors.fill: parent
            RotationAnimator on rotation { running: control.running && control.visible && !Tokens.reduceMotion; from: 0; to: 360; duration: 900; loops: Animation.Infinite }
            Rectangle {
                width: Math.max(2, parent.width / 10) * 1.6
                height: width
                radius: width / 2
                x: (parent.width - width) / 2
                y: 0
                color: Tokens.accent
            }
        }
    }
}
