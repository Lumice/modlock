import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.Popup {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, contentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, contentHeight + topPadding + bottomPadding)
    padding: 12
    background: Rectangle {
        radius: Tokens.radius
        color: Tokens.surface2
        border.color: Tokens.borderStrong
    }
    T.Overlay.modal: Rectangle { color: Tokens.scrim }
    T.Overlay.modeless: Rectangle { color: "transparent" }
}
