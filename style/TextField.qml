import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.TextField {
    id: control
    implicitWidth: implicitBackgroundWidth + leftInset + rightInset
                   || Math.max(contentWidth, placeholder.implicitWidth) + leftPadding + rightPadding
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             contentHeight + topPadding + bottomPadding,
                             placeholder.implicitHeight + topPadding + bottomPadding)
    padding: 7
    leftPadding: 12
    rightPadding: 12
    color: enabled ? Tokens.text : Tokens.textDisabled
    selectionColor: Tokens.accent
    selectedTextColor: Tokens.accentText
    placeholderTextColor: Tokens.textFaint
    verticalAlignment: TextInput.AlignVCenter
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
        implicitWidth: 180
        implicitHeight: Tokens.controlHeight
        radius: Tokens.radius
        color: control.enabled ? Tokens.field : Tokens.surface1
        border.width: control.activeFocus ? 2 : 1
        border.color: control.activeFocus ? Tokens.focus : Tokens.fieldBorder
    }
}
