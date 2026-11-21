import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.TextArea {
    id: control
    implicitWidth: Math.max(contentWidth + leftPadding + rightPadding,
                            implicitBackgroundWidth + leftInset + rightInset,
                            placeholder.implicitWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(contentHeight + topPadding + bottomPadding,
                             implicitBackgroundHeight + topInset + bottomInset,
                             placeholder.implicitHeight + topPadding + bottomPadding)
    padding: 8
    leftPadding: 12
    rightPadding: 12
    color: enabled ? Tokens.text : Tokens.textDisabled
    selectionColor: Tokens.accent
    selectedTextColor: Tokens.accentText
    placeholderTextColor: Tokens.textFaint
    PlaceholderText {
        id: placeholder
        x: control.leftPadding
        y: control.topPadding
        width: control.width - (control.leftPadding + control.rightPadding)
        height: control.height - (control.topPadding + control.bottomPadding)
        text: control.placeholderText
        font: control.font
        color: control.placeholderTextColor
        verticalAlignment: control.verticalAlignment
        visible: !control.length && !control.preeditText && (!control.activeFocus || control.horizontalAlignment !== Qt.AlignHCenter)
        elide: Text.ElideRight
        renderType: control.renderType
    }
    background: Rectangle {
        implicitWidth: 200
        implicitHeight: 60
        radius: Tokens.radiusSmall
        color: control.enabled ? Tokens.field : Tokens.surface1
        border.width: control.activeFocus ? 2 : 1
        border.color: control.activeFocus ? Tokens.focus : control.hovered ? Tokens.borderStrong : Tokens.border
    }
}
