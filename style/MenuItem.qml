import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.MenuItem {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    // Hidden items collapse instead of leaving gaps in the menu.
    implicitHeight: !visible ? 0 : Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    padding: 6
    leftPadding: 10
    rightPadding: 10
    spacing: 8
    icon.width: 16
    icon.height: 16
    icon.color: control.enabled ? Tokens.text : Tokens.textDisabled
    contentItem: IconLabel {
        readonly property real arrowPadding: control.subMenu && control.arrow ? control.arrow.width + control.spacing : 0
        readonly property real indicatorPadding: control.checkable && control.indicator ? control.indicator.width + control.spacing : 0
        leftPadding: !control.mirrored ? indicatorPadding : arrowPadding
        rightPadding: control.mirrored ? indicatorPadding : arrowPadding
        spacing: control.spacing
        mirrored: control.mirrored
        display: control.display
        alignment: Qt.AlignLeft
        icon: control.icon
        text: control.text
        font: control.font
        color: control.enabled ? Tokens.text : Tokens.textDisabled
    }
    indicator: ColorImage {
        x: control.mirrored ? control.width - width - control.rightPadding : control.leftPadding
        y: control.topPadding + (control.availableHeight - height) / 2
        width: 14
        height: 14
        visible: control.checkable && control.checked
        sourceSize: Qt.size(14, 14)
        color: Tokens.accentInk
        source: "qrc:/qt/qml/DeadlockStyle/icons/check.svg"
    }
    arrow: ColorImage {
        x: control.mirrored ? control.leftPadding : control.width - width - control.rightPadding
        y: control.topPadding + (control.availableHeight - height) / 2
        width: 14
        height: 14
        visible: control.subMenu
        mirror: control.mirrored
        rotation: -90
        sourceSize: Qt.size(14, 14)
        color: Tokens.textMuted
        source: "qrc:/qt/qml/DeadlockStyle/icons/chevron-down.svg"
    }
    background: Rectangle {
        implicitWidth: 200
        implicitHeight: 32
        radius: Tokens.radiusSmall
        color: control.down ? Tokens.surface4 : control.highlighted ? Tokens.surface3 : "transparent"
    }
}
