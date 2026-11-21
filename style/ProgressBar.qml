import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.ProgressBar {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, implicitContentHeight + topPadding + bottomPadding)
    contentItem: Item {
        implicitWidth: 160
        implicitHeight: 6
        clip: true
        Rectangle {
            width: control.indeterminate ? parent.width * 0.3 : control.visualPosition * parent.width
            height: parent.height
            radius: 3
            color: Tokens.accent
            NumberAnimation on x {
                running: control.indeterminate && control.visible && !Tokens.reduceMotion
                from: -control.width * 0.3
                to: control.width
                duration: 1200
                loops: Animation.Infinite
            }
        }
    }
    background: Rectangle {
        implicitWidth: 160
        implicitHeight: 6
        radius: 3
        color: Tokens.surface3
    }
}
