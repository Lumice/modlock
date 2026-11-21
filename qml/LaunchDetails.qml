pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "LaunchPresentation.js" as LaunchPresentation
import "FontPresentation.js" as FontPresentation

Dialog {
    id: root
    required property LibraryModel library
    readonly property LaunchController controller: library.launch
    readonly property var displayedReady: controller.review.summary ? controller.review : controller.attempt.ready || ({})
    readonly property var displayedIdentity: controller.review.summary || !displayedReady.summary
        ? ({mode: controller.mode, profileName: controller.targetProfileName})
        : LaunchPresentation.identity(displayedReady, library.profiles)
    readonly property var barrier: controller.barriers.length === 1 ? controller.barriers[0] : ({})
    readonly property bool needsSteamAcknowledgement: barrier.stage === "dispatchIntent"
    property bool summaryOpen: false
    signal dismissed()
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(690, parent ? parent.width - 32 : 690)
    height: Math.min(690, parent ? parent.height - 32 : 690)
    title: qsTr("Launch details")
    modal: false
    closePolicy: Popup.CloseOnEscape
    standardButtons: Dialog.Close
    Component.onCompleted: { open(); standardButton(Dialog.Close).forceActiveFocus() }
    onClosed: dismissed()
    ColumnLayout {
        anchors.fill: parent
        spacing: 8
        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                width: parent.width
                spacing: 8
                Label {
                    Layout.fillWidth: true
                    text: LaunchPresentation.phase(root.controller.phase)
                    font.weight: Font.Medium
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    text: root.displayedIdentity.profileName ? qsTr("%1 · Profile: %2").arg(LaunchPresentation.mode(root.displayedIdentity.mode)).arg(root.displayedIdentity.profileName) : qsTr("No launch selected")
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Label {
                    Layout.fillWidth: true
                    visible: !root.needsSteamAcknowledgement
                    text: root.controller.preparationOutcome === "succeeded" ? qsTr("File preparation succeeded.")
                        : root.controller.preparationOutcome === "failed" ? qsTr("File preparation failed.") : qsTr("File preparation outcome has not been confirmed.")
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: !root.needsSteamAcknowledgement
                    text: qsTr("%1\n%2\n%3\n%4")
                        .arg(LaunchPresentation.preparation(root.controller.preparationState))
                        .arg(LaunchPresentation.handoff(root.controller.handoffState))
                        .arg(LaunchPresentation.observation(root.controller.observationState))
                        .arg(LaunchPresentation.game(root.controller.gameStatus))
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!root.controller.progress.kind
                    text: LaunchPresentation.progress(root.controller.progress)
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!root.controller.error || !!root.controller.preparationError
                    text: qsTr("Preparation error: %1\nLaunch error: %2").arg(Theme.friendlyError(root.controller.preparationError) || qsTr("None reported")).arg(Theme.friendlyError(root.controller.error) || qsTr("None reported"))
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Label {
                    Layout.fillWidth: true
                    visible: !root.needsSteamAcknowledgement
                    text: qsTr("Cancel stops a request before handoff. Stop waiting ends manager observation only. Neither closes Steam or Deadlock, nor restores the previous mod setup.")
                    wrapMode: Text.Wrap
                }
                Button {
                    id: summaryButton
                    text: qsTr("Preparation details…")
                    visible: !!root.displayedReady.summary
                    onClicked: root.summaryOpen = true
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!(root.controller.gameStatus.stop || {}).status
                    text: LaunchPresentation.stop(root.controller.gameStatus.stop || ({}))
                    wrapMode: Text.Wrap
                }
                ListView {
                    id: processes
                    Layout.fillWidth: true
                    Layout.preferredHeight: (root.controller.gameStatus.processes || []).length > 0 ? 180 : 0
                    clip: true
                    reuseItems: true
                    activeFocusOnTab: true
                    keyNavigationEnabled: false
                    Accessible.name: qsTr("Observed Deadlock processes")
                    Accessible.description: qsTr("Use arrow keys to scroll and Tab to reach each available Stop button.")
                    Keys.onPressed: event => FontPresentation.scrollKey(processes, event, Math.max(24, Theme.body * 2))
                    model: root.controller.gameStatus.processes || []
                    ScrollBar.vertical: ScrollBar {}
                    Rectangle {
                        parent: processes
                        anchors.fill: parent
                        color: "transparent"
                        border.color: Theme.accent
                        border.width: processes.activeFocus ? 2 : 0
                        Accessible.ignored: true
                    }
                    delegate: RowLayout {
                        id: processRow
                        required property var modelData
                        required property int index
                        width: processes.width
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Deadlock process %1\n%2").arg(processRow.index + 1).arg(LaunchPresentation.stopAvailability(processRow.modelData.stopAvailability))
                            wrapMode: Text.Wrap
                        }
                        Button {
                            text: qsTr("Stop")
                            enabled: root.controller.canStop && processRow.modelData.canStop
                            onClicked: root.controller.stopGame(processRow.modelData.sessionToken)
                            Accessible.name: qsTr("Stop Deadlock process %1").arg(processRow.index + 1)
                            ToolTip.visible: hovered || activeFocus
                            ToolTip.text: qsTr("Request a graceful game exit; wait up to 10 seconds for confirmation.")
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: (root.controller.barriers.length > 0 || root.controller.writeGate) && root.controller.gameStatus.state !== "running"
                    text: root.needsSteamAcknowledgement
                        ? qsTr("Waiting for Steam to start Deadlock. You can change the setup when waiting ends.")
                        : qsTr("Check again to see whether Deadlock has closed. This won’t start the game again.")
                    wrapMode: Text.Wrap
                }
                Button {
                    Layout.fillWidth: true
                    visible: root.controller.barriers.length > 0 && !root.needsSteamAcknowledgement
                    text: qsTr("Release setup")
                    enabled: root.controller.canResolve
                    onClicked: root.controller.resolveBarrier(false)
                }
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            Button {
                text: root.controller.canStopWaiting || root.needsSteamAcknowledgement ? qsTr("Stop waiting") : qsTr("Cancel")
                visible: root.controller.canCancel || root.controller.canStopWaiting || (root.needsSteamAcknowledgement && root.controller.phase !== "running")
                onClicked: root.controller.canCancel || root.controller.canStopWaiting ? root.controller.cancel() : root.controller.retire()
            }
            Button {
                text: qsTr("Check again")
                visible: !root.needsSteamAcknowledgement
                enabled: root.controller.available && !root.controller.busy
                onClicked: root.controller.checkAgain()
            }
            Button {
                text: qsTr("Close connection")
                visible: !root.needsSteamAcknowledgement
                enabled: root.controller.available && !root.controller.busy && !root.library.busy
                onClicked: root.controller.retire()
                ToolTip.visible: hovered || activeFocus
                ToolTip.text: qsTr("Closes the manager connection. Steam and Deadlock keep running; launch is never retried.")
            }
        }
    }
    Loader {
        active: root.summaryOpen && root.visible
        sourceComponent: Dialog {
            parent: Overlay.overlay
            anchors.centerIn: parent
            width: Math.min(650, parent ? parent.width - 48 : 650)
            height: Math.min(590, parent ? parent.height - 48 : 590)
            modal: true
            title: qsTr("Preparation details")
            standardButtons: Dialog.Close
            Component.onCompleted: open()
            onClosed: { root.summaryOpen = false; summaryButton.forceActiveFocus() }
            contentItem: LaunchSummary { controller: root.controller; profiles: root.library.profiles; installedFonts: root.library.fonts.installedOwners.length > 0 }
        }
    }
}
