import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.Menu {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset, contentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset, contentHeight + topPadding + bottomPadding)
    margins: 0
    padding: 4
    overlap: 1
    delegate: MenuItem { }
    contentItem: ListView {
        implicitHeight: contentHeight
        model: control.contentModel
        interactive: Window.window ? contentHeight + control.topPadding + control.bottomPadding > control.height : false
        clip: true
        currentIndex: control.currentIndex
        T.ScrollIndicator.vertical: ScrollIndicator { }
    }
    background: Rectangle {
        implicitWidth: 260
        implicitHeight: 32
        radius: Tokens.radius
        color: Tokens.surface2
        border.color: Tokens.borderStrong
    }
    enter: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Tokens.fast; easing.type: Easing.OutCubic } }
    exit: Transition { NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Tokens.fast; easing.type: Easing.InCubic } }
    T.Overlay.modal: Rectangle { color: Tokens.scrim }
    T.Overlay.modeless: Rectangle { color: "transparent" }
}
