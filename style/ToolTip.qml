import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.ToolTip {
    id: control
    x: parent ? (parent.width - implicitWidth) / 2 : 0
    y: -implicitHeight - 6
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, contentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, contentHeight + topPadding + bottomPadding)
    margins: 6
    padding: 6
    leftPadding: 10
    rightPadding: 10
    delay: 450
    closePolicy: T.Popup.CloseOnEscape | T.Popup.CloseOnPressOutsideParent | T.Popup.CloseOnReleaseOutsideParent
    palette.toolTipText: Tokens.text
    font.pointSize: Tokens.caption
    contentItem: Text {
        text: control.text
        font: control.font
        wrapMode: Text.Wrap
        color: Tokens.text
    }
    background: Rectangle {
        radius: Tokens.radiusSmall
        color: Tokens.surface3
        border.color: Tokens.borderStrong
    }
    enter: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Tokens.fast } }
    exit: Transition { NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Tokens.fast } }
}
