pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls.impl
import DeadlockStyle

T.ComboBox {
    id: control
    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    leftPadding: 12
    rightPadding: 12 + (indicator ? indicator.width + spacing : 0)
    spacing: 6
    delegate: ItemDelegate {
        required property var model
        required property int index
        width: ListView.view.width
        text: model[control.textRole]
        highlighted: control.highlightedIndex === index
        hoverEnabled: control.hoverEnabled
    }
    indicator: ColorImage {
        x: control.mirrored ? control.padding : control.width - width - 10
        y: control.topPadding + (control.availableHeight - height) / 2
        width: 14
        height: 14
        sourceSize: Qt.size(14, 14)
        color: control.enabled ? Tokens.textMuted : Tokens.textDisabled
        source: "qrc:/qt/qml/DeadlockStyle/icons/chevron-down.svg"
    }
    contentItem: T.TextField {
        leftPadding: 0
        rightPadding: 0
        text: control.editable ? control.editText : control.displayText
        enabled: control.editable
        autoScroll: control.editable
        readOnly: control.down
        inputMethodHints: control.inputMethodHints
        validator: control.validator
        selectByMouse: control.selectTextByMouse
        color: control.enabled ? Tokens.text : Tokens.textDisabled
        selectionColor: Tokens.accent
        selectedTextColor: Tokens.accentText
        verticalAlignment: Text.AlignVCenter
        font: control.font
    }
    background: Rectangle {
        implicitWidth: 120
        implicitHeight: Tokens.controlHeight
        radius: Tokens.radius
        color: control.down ? Tokens.surface4 : control.hovered ? Tokens.surface3 : Tokens.surface2
        border.width: control.visualFocus ? 2 : 1
        border.color: control.visualFocus ? Tokens.focus : Tokens.fieldBorder
    }
    popup: T.Popup {
        y: control.height + 4
        width: Math.max(control.width, 160)
        height: Math.min(contentItem.implicitHeight + topPadding + bottomPadding,
                         9 * Tokens.controlHeight + topPadding + bottomPadding,
                         control.contentItem.Window.height - topMargin - bottomMargin)
        topMargin: 6
        bottomMargin: 6
        padding: 4
        contentItem: ListView {
            id: popupList
            clip: true
            implicitHeight: contentHeight
            model: control.delegateModel
            currentIndex: control.highlightedIndex
            highlightMoveDuration: 0
            T.ScrollBar.vertical: ScrollBar {
                policy: popupList.contentHeight > popupList.height ? T.ScrollBar.AlwaysOn : T.ScrollBar.AlwaysOff
            }
        }
        background: Rectangle {
            radius: Tokens.radius
            color: Tokens.surface2
            border.color: Tokens.borderStrong
        }
        enter: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Tokens.fast } }
    }
}
