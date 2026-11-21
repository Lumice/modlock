import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

ColumnLayout {
    id: root
    property string headline
    property string text
    property string actionText: ""
    signal actionTriggered()
    // The emblem is the only motion on an empty page: the rays turn about once a
    // minute and the eye breathes. Both stop while hidden or with reduced motion,
    // and settle back to their resting pose.
    readonly property bool animated: visible && !Tokens.reduceMotion
    spacing: Theme.spacing12
    Item {
        objectName: "emptyStateEmblem"
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 96
        Layout.preferredHeight: 96
        Accessible.ignored: true
        ColorImage {
            id: rays
            objectName: "emptyStateRays"
            anchors.fill: parent
            source: Theme.icon("sunburst-rays")
            sourceSize: Qt.size(96, 96)
            color: Qt.alpha(Theme.brass, 0.42)
            Accessible.ignored: true
            RotationAnimator on rotation {
                id: turn
                from: 0
                to: 360
                duration: 60000
                loops: Animation.Infinite
                running: root.animated
                onRunningChanged: if (!running) rays.rotation = 0
            }
        }
        Rectangle {
            // A faint teal halo behind the eye; static, so it never reads as a status light.
            anchors.centerIn: parent
            width: 46
            height: 46
            radius: width / 2
            color: Qt.alpha(Theme.live, 0.10)
            Accessible.ignored: true
        }
        Rectangle {
            id: eye
            objectName: "emptyStateEye"
            anchors.centerIn: parent
            width: 32
            height: 32
            radius: width / 2
            color: "transparent"
            border.width: 2
            border.color: Theme.brass
            Accessible.ignored: true
            Rectangle {
                id: pupil
                objectName: "emptyStatePupil"
                anchors.centerIn: parent
                width: 12
                height: 12
                radius: width / 2
                color: Theme.live
            }
            SequentialAnimation {
                running: root.animated
                loops: Animation.Infinite
                onRunningChanged: if (!running) pupil.opacity = 1
                OpacityAnimator { target: pupil; from: 1; to: 0.45; duration: 1800; easing.type: Easing.InOutSine }
                OpacityAnimator { target: pupil; from: 0.45; to: 1; duration: 1800; easing.type: Easing.InOutSine }
            }
        }
    }
    Label {
        Layout.fillWidth: true
        text: root.headline
        textFormat: Text.PlainText
        font.family: Theme.displayFamily
        font.pointSize: Theme.title
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }
    Label {
        Layout.fillWidth: true
        visible: root.text !== ""
        text: root.text
        textFormat: Text.PlainText
        color: Theme.secondary
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }
    Button {
        Layout.alignment: Qt.AlignHCenter
        visible: root.actionText !== ""
        text: root.actionText
        onClicked: root.actionTriggered()
    }
}
