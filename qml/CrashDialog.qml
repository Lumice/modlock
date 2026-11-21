pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
Dialog {
    id: root
    required property LibraryModel library
    readonly property LaunchHealth health: library.launch.health
    readonly property var report: health.report
    signal findBrokenMod()
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(640, parent ? parent.width - 32 : 640)
    height: Math.min(560, parent ? parent.height - 32 : 560)
    modal: true
    title: report.confirmed ? qsTr("What changed since your last normal session") : qsTr("Did Deadlock crash?")
    closePolicy: Popup.NoAutoClose
    Component.onCompleted: open()
    contentItem: ColumnLayout {
        Label {
            Layout.fillWidth: true
            text: root.report.confirmed ? qsTr("These changes may help explain the crash. Review changes before applying them.")
                : root.report.early ? qsTr("Deadlock closed within five minutes. Steam does not tell us whether the game crashed.")
                : qsTr("Deadlock closed. Was this a normal session? Your answer helps compare future launches.")
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.report.bisectAnswered
            text: qsTr("Answered Still broken for the launched search round. You can change this answer before continuing.")
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Button {
            visible: !!root.report.bisectAnswered
            text: qsTr("Change answer")
            enabled: !root.library.busy
            onClicked: root.health.changeBisectAnswer()
        }
        Label {
            visible: !root.report.hasBaseline
            text: qsTr("No normal session recorded yet. Changes will appear after you confirm one.")
            Layout.fillWidth: true
            wrapMode: Text.Wrap
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            ColumnLayout {
                width: parent.width
                Repeater {
                    model: root.report.changes || []
                    RowLayout {
                        required property var modelData
                        required property int index
                        Layout.fillWidth: true
                        Label { Layout.fillWidth: true; text: modelData.text; textFormat: Text.PlainText; wrapMode: Text.Wrap }
                        Button {
                            text: modelData.kind === "variants" || modelData.kind === "order" ? qsTr("Review…") : qsTr("Undo")
                            enabled: !!root.report.confirmed && !root.library.busy && modelData.kind !== "generation" && modelData.kind !== "gameBuild" && modelData.kind !== "mode"
                            onClicked: root.health.undo(index)
                        }
                    }
                }
            }
        }
        Label { Layout.fillWidth: true; visible: !!text; text: root.health.error; textFormat: Text.PlainText; wrapMode: Text.Wrap }
        Flow {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            Button { objectName: "confirmCrashButton"; visible: !root.report.confirmed; text: qsTr("Yes, it crashed"); onClicked: root.health.answer(true) }
            Button { objectName: "normalExitButton"; visible: !root.report.confirmed; text: qsTr("No, I closed it"); onClicked: root.health.answer(false) }
            Button { visible: !!root.report.confirmed; text: qsTr("Find a broken mod…"); enabled: root.library.canStartBisect; onClicked: { root.health.dismiss(); root.findBrokenMod() } }
            Button { text: qsTr("Dismiss"); onClicked: root.health.dismiss() }
        }
    }
}
