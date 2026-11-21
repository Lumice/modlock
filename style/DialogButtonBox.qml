import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.DialogButtonBox {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            (control.count === 1 ? implicitContentWidth * 2 : implicitContentWidth) + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding)
    contentWidth: (contentItem as ListView)?.contentWidth ?? 0
    spacing: 8
    padding: 16
    leftPadding: 16
    rightPadding: 16
    alignment: Qt.AlignRight
    delegate: Button {
        width: Math.max(implicitWidth, 88)
        highlighted: DialogButtonBox.buttonRole === DialogButtonBox.AcceptRole || DialogButtonBox.buttonRole === DialogButtonBox.YesRole
    }
    contentItem: ListView {
        implicitWidth: contentWidth
        model: control.contentModel
        spacing: control.spacing
        orientation: ListView.Horizontal
        boundsBehavior: Flickable.StopAtBounds
        snapMode: ListView.SnapToItem
    }
    background: Rectangle {
        implicitHeight: 56
        color: "transparent"
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Tokens.border; visible: control.position === T.DialogButtonBox.Footer }
    }
}
