import QtQuick
import QtQuick.Templates as T
import DeadlockStyle

T.Label {
    color: enabled ? Tokens.text : Tokens.textDisabled
    linkColor: Tokens.accentInk
}
