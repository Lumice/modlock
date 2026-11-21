import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.ScrollIndicator {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, implicitContentHeight + topPadding + bottomPadding)
    padding: 2
    contentItem: Rectangle {
        implicitWidth: 4
        implicitHeight: 4
        radius: 2
        color: Tokens.borderStrong
        opacity: control.active ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: Tokens.normal } }
    }
}
