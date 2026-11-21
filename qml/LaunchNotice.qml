pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "LaunchPresentation.js" as LaunchPresentation

Pane {
    id: root
    objectName: "launchNotice"
    required property LaunchController controller
    required property bool canReviewApply
    signal detailsRequested()
    signal applyRequested()
    property bool occupied: false
    property bool pendingApplyReview: false
    readonly property var barrier: controller.barriers.length === 1 ? controller.barriers[0] : null
    readonly property bool legacy: !!barrier && barrier.legacy === true
    readonly property bool needsClear: legacy || (!!barrier && controller.gameStatus.state === "idle"
        && !controller.canStopWaiting && !controller.busy)
    readonly property string errorText: Theme.friendlyError(controller.preparationError || controller.error)
    visible: !occupied && (needsClear || errorText !== "" || controller.phase === "failed" || controller.phase === "timedOut")
    padding: Theme.spacing8
    background: Rectangle {
        radius: Theme.radius4
        color: root.errorText !== "" ? Theme.dangerSoft : Theme.surfaceLow
        border.color: root.errorText !== "" ? Theme.danger : Theme.rule
    }
    function continueApplyReview() {
        if (!pendingApplyReview) return
        if (!controller.writeGate && canReviewApply) { pendingApplyReview = false; applyRequested() }
        else if (controller.writeGate && !controller.busy) controller.retire()
    }
    onCanReviewApplyChanged: if (pendingApplyReview) Qt.callLater(continueApplyReview)
    Connections {
        target: root.controller
        function onChanged() { if (root.pendingApplyReview) Qt.callLater(root.continueApplyReview) }
    }
    ColumnLayout {
        anchors.fill: parent
        spacing: Theme.spacing4
        RowLayout {
            Layout.fillWidth: true
            Label {
                objectName: "launchNoticeText"
                Layout.fillWidth: true
                text: root.needsClear && root.errorText !== "" ? qsTr("Couldn't clear it: %1").arg(root.errorText)
                    : root.legacy ? LaunchPresentation.legacyMessage(root.barrier.markerTime)
                    : root.needsClear ? qsTr("An earlier launch didn't finish. Clear it to change your mods. Steam may still start Deadlock.")
                    : root.errorText || qsTr("Deadlock didn't start. Open launch details to check what happened.")
                color: Theme.text
                font.pointSize: Theme.small
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Button {
                objectName: "clearLegacyLaunchButton"
                visible: root.needsClear
                text: qsTr("Clear it")
                enabled: root.controller.canResolve
                onClicked: root.controller.resolveBarrier(root.barrier.stage === "dispatchIntent")
            }
            Button {
                objectName: "launchReviewChangesButton"
                visible: !root.needsClear && root.errorText.includes("Review the changes to continue.")
                text: qsTr("Review changes…")
                onClicked: { root.pendingApplyReview = true; root.continueApplyReview() }
            }
            Button { visible: !root.needsClear; text: qsTr("Details…"); onClicked: root.detailsRequested() }
        }

    }
}
