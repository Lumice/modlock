import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.Dialog {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            contentWidth + leftPadding + rightPadding,
                            implicitHeaderWidth,
                            implicitFooterWidth)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             contentHeight + topPadding + bottomPadding
                             + (implicitHeaderHeight > 0 ? implicitHeaderHeight + spacing : 0)
                             + (implicitFooterHeight > 0 ? implicitFooterHeight + spacing : 0))
    padding: 20
    topPadding: 8
    modal: true
    background: Rectangle {
        radius: Tokens.radiusLarge
        color: Tokens.surface1
        border.color: Tokens.borderStrong
    }
    header: Item {
        visible: !!control.title
        implicitWidth: titleLabel.implicitWidth
        implicitHeight: titleMetrics.height + titleLabel.topPadding + titleLabel.bottomPadding
        FontMetrics { id: titleMetrics; font: titleLabel.font }
        Label {
            id: titleLabel
            anchors.fill: parent
            text: control.title
            elide: Label.ElideRight
            font.pointSize: Tokens.title
            font.weight: Font.DemiBold
            padding: 20
            bottomPadding: 8
            background: Item { }
        }
    }
    footer: DialogButtonBox {
        visible: count > 0
    }
    enter: Transition {
        ParallelAnimation {
            NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Tokens.panel; easing.type: Easing.OutCubic }
            NumberAnimation { property: "scale"; from: 0.98; to: 1; duration: Tokens.panel; easing.type: Easing.OutCubic }
        }
    }
    exit: Transition { NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Tokens.panel; easing.type: Easing.InCubic } }
    T.Overlay.modal: Rectangle { color: Tokens.scrim; Behavior on opacity { NumberAnimation { duration: Tokens.panel } } }
    T.Overlay.modeless: Rectangle { color: "transparent" }
}
