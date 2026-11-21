pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// Compact game-setup state for the viewed profile, shown in the library header.
RowLayout {
    id: root
    required property LibraryModel library
    signal reviewRequested()
    readonly property var activeProfile: library.profiles.find(profile => profile.id === library.activeProfileId)
    readonly property string appliedName: (library.profiles.find(profile => profile.id === library.appliedProfileId) || {}).name || library.appliedProfileId
    // A leftover recoverable error (e.g. gameRunning from an earlier review) must not
    // hide the review shortcut; canPreviewApply decides whether a new attempt is allowed.
    readonly property bool settledSnapshot: library.nativeMode && !library.loading && !library.busy && !library.writeBusy
                                            && !library.outcomeUnknown && !library.recoveryRequired && !library.launch.writeGate
    readonly property bool stableSnapshot: settledSnapshot && !library.error
    readonly property string state: !library.nativeMode ? ""
        : library.loading ? "loading"
        : library.outcomeUnknown ? "check"
        : library.recoveryNeedsAttention ? "recovery"
        : !stableSnapshot ? ""
        : library.pendingStatus === "selectionRequired" ? "choose"
        : library.pendingStatus === "unknown" || library.pendingStatus === "" ? "unknown"
        : !library.appliedProfileId ? "never"
        : library.appliedProfileId !== library.viewedProfileId ? "other"
        : library.pendingStatus === "pending" || library.pendingChanges ? "pending"
        : library.pendingStatus === "current" ? "current" : "unknown"
    readonly property bool canReviewPending: library.pendingChanges && settledSnapshot && library.canPreviewApply
    spacing: Theme.spacing8
    Label {
        objectName: "activeProfileLabel"
        visible: !root.library.nativeMode && !root.library.demoMode && !!root.activeProfile
        text: qsTr("Active: %1").arg(root.activeProfile ? root.activeProfile.name : "")
        textFormat: Text.PlainText
        font.pointSize: Theme.small
        color: Theme.secondary
    }
    ColumnLayout {
        spacing: 0
        StatusPill {
            id: pending
            objectName: "pendingLabel"
            visible: text !== "" && !["current", "pending", "never", "other", "loading"].includes(root.state)
            text: root.state === "loading" ? qsTr("Loading profile…")
                : root.state === "check" ? qsTr("Check the previous change")
                : root.state === "recovery" ? qsTr("Recovery needed")
                : root.state === "choose" ? qsTr("Some mods need file choices")
                : root.state === "unknown" ? qsTr("Game setup needs checking")
                : root.state === "never" ? qsTr("Not applied to the game yet")
                : root.state === "other" ? qsTr("Game uses %1").arg(root.appliedName)
                : root.state === "pending" ? qsTr("Changes not applied")
                : root.state === "current" ? qsTr("Setup applied") : ""
            tone: root.state === "current" ? "success"
                : root.state === "pending" ? "accent"
                : root.state === "recovery" ? "danger"
                : root.state === "loading" || root.state === "other" || root.state === "never" ? "muted" : "warning"
            HoverHandler { id: pendingHover }
            ToolTip {
                visible: pendingHover.hovered
                text: qsTr("Changes are saved when Deadlock is closed, or when you next play.")
                width: Math.min(360, implicitWidth)
            }
        }
        Label {
            visible: root.canReviewPending && (!root.library.automaticApply || ["unknown", "choose"].includes(root.state))
            text: qsTr("Profile updated · game unchanged")
            font.pointSize: Theme.small
            color: Theme.secondary
        }
    }
    Button {
        objectName: "reviewPendingChangesButton"
        visible: root.canReviewPending && (!root.library.automaticApply || ["unknown", "choose"].includes(root.state))
        text: qsTr("Review changes…")
        onClicked: root.reviewRequested()
    }
}
