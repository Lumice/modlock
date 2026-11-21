import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.CheckBox {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    padding: 4
    spacing: 8
    indicator: Rectangle {
        implicitWidth: 18
        implicitHeight: 18
        x: control.text ? (control.mirrored ? control.width - width - control.rightPadding : control.leftPadding) : control.leftPadding + (control.availableWidth - width) / 2
        y: control.topPadding + (control.availableHeight - height) / 2
        radius: 4
        color: control.checkState !== Qt.Unchecked ? (control.enabled ? Tokens.accent : Tokens.surface4)
             : control.down ? Tokens.surface3 : Tokens.field
        border.width: control.checkState !== Qt.Unchecked ? 0 : 1
        border.color: control.hovered ? Tokens.textFaint : Tokens.borderStrong
        Rectangle {
            objectName: "checkFocusRing"
            anchors.fill: parent
            anchors.margins: -4
            radius: parent.radius + 4
            color: "transparent"
            border.width: 2
            border.color: Tokens.focus
            visible: control.visualFocus
        }
        ColorImage {
            x: (parent.width - width) / 2
            y: (parent.height - height) / 2
            width: 12
            height: 12
            sourceSize: Qt.size(12, 12)
            color: Tokens.accentText
            source: "qrc:/qt/qml/DeadlockStyle/icons/check.svg"
            visible: control.checkState === Qt.Checked
        }
        Rectangle {
            x: (parent.width - width) / 2
            y: (parent.height - height) / 2
            width: 10
            height: 2
            radius: 1
            color: Tokens.accentText
            visible: control.checkState === Qt.PartiallyChecked
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
