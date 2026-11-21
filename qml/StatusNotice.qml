pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

Rectangle {
    id: root
    objectName: "statusNotice"
    // Qt 6.12 AOT crashes on the nested LibraryModel lookups in this component.
    required property var library
    readonly property var bulkSummary: library.lastBulkResult || ({})
    property string dismissedBulkKey: ""
    onBulkSummaryChanged: if (bulkSummary.changed === undefined) dismissedBulkKey = ""
    property bool importActive: false
    readonly property string bulkKey: JSON.stringify(bulkSummary)
    readonly property bool hasBulkResult: bulkSummary.changed !== undefined && bulkKey !== dismissedBulkKey && !importActive && checkMessage === ""
    readonly property var healthReport: (library.health || {}).report || ({})
    readonly property var repairable: (healthReport.mods || []).filter(m => ["fixable", "partlyFixable", "originalFiles"].includes(m.status))
    // Update/fix check results belong to the Library page, next to its Check button.
    property bool showChecks: true
    readonly property string checkMessage: !showChecks ? "" : library.updatesError ? Theme.friendlyError(library.updatesError) : (library.health || {}).error ? qsTr("Couldn't check mods. Try again.") : library.updatesChecking || library.healthChecking ? qsTr("Checking updates and fixes…") : library.updatesOffline ? qsTr("Couldn't reach GameBanana; updates weren't checked.") : (healthReport.unavailableMods || []).length ? qsTr("Some mods couldn't be checked because their files are unavailable.") : repairable.length === 1 ? qsTr("1 mod has fixes for this patch") : repairable.length ? qsTr("%1 mods have fixes for this patch").arg(repairable.length) : ""
    property bool occupied: false
    signal installedRequested
    signal applyRequested
    Layout.fillWidth: true
    // Derived from state, not child visibility (children report false while this is hidden).
    // Recovery, errors and work take priority; normal launch progress stays on Play.
    readonly property bool healthyHandoff: root.library.recoveryRequired && !root.library.recoveryNeedsAttention
    readonly property bool showOperation: (!root.library.launch.writeGate && root.library.busy) || root.library.outcomeUnknown || root.library.recoveryNeedsAttention || root.library.externalGameinfoChange
    readonly property bool externalChangeShown: visible && !occupied && showOperation && !library.recoveryNeedsAttention && !healthyHandoff && library.externalGameinfoChange
    readonly property bool showBootstrap: !root.library.demoMode && !root.library.nativeMode && root.library.bootstrapReason !== "" && root.library.bootstrapState !== "absent" && root.library.bootstrapState !== "pending" && root.library.bootstrapState !== "creating" && root.library.bootstrapState !== "confirming"
    visible: root.library.warnings.length > 0 || (!occupied && (root.library.error !== "" || showOperation || showBootstrap || hasBulkResult || checkMessage !== ""))
    implicitHeight: noticeColumn.implicitHeight + Theme.spacing24
    radius: Theme.radius
    readonly property string tone: root.library.error !== "" || root.library.recoveryNeedsAttention ? "danger" : root.library.outcomeUnknown || root.library.externalGameinfoChange || root.library.warnings.length > 0 ? "warning" : ""
    color: tone === "danger" ? Theme.dangerSoft : tone === "warning" ? Theme.warningSoft : Theme.surfaceLow
    border.color: tone === "danger" ? Qt.alpha(Theme.danger, 0.4) : tone === "warning" ? Qt.alpha(Theme.warning, 0.4) : Theme.rule
    RowLayout {
        anchors.fill: parent
        anchors.margins: Theme.spacing12
        spacing: Theme.spacing12
        ColorImage {
            Layout.alignment: Qt.AlignTop
            Layout.preferredWidth: 18
            Layout.preferredHeight: 18
            source: Theme.icon(root.tone === "" ? "info" : "alert")
            sourceSize: Qt.size(18, 18)
            color: root.tone === "danger" ? Theme.danger : root.tone === "warning" ? Theme.warning : Theme.secondary
        }
        ColumnLayout {
            id: noticeColumn
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            spacing: Theme.spacing8
            Label {
                id: bulkResult
                objectName: "bulkResultLabel"
                Layout.fillWidth: true
                readonly property var result: root.library.lastBulkResult || ({})
                readonly property var skipped: result.skipped || []
                visible: root.hasBulkResult && !root.library.error && !root.library.busy && !root.library.outcomeUnknown && !root.library.recoveryRequired && root.library.warnings.length === 0 && !root.showBootstrap && !root.library.externalGameinfoChange
                text: skipped.length === 0 ? result.changed === 1 ? qsTr("1 mod updated.") : qsTr("%1 mods updated.").arg(result.changed || 0) : qsTr("%1 updated, %2 skipped (%3).").arg(result.changed || 0).arg(skipped.length).arg(Array.from(new Set(skipped.map(s => s.reason === "invalidLocalSelection" || s.reason === "selectionRequired" ? qsTr("needs file choices") : qsTr("could not be changed")))).join(", "))
                color: skipped.length ? Theme.warning : Theme.secondary
                font.pointSize: Theme.small
                wrapMode: Text.Wrap
            }
            Button {
                objectName: "dismissBulkResult"
                visible: root.hasBulkResult && !root.showOperation && !root.showBootstrap && !root.library.error && root.library.warnings.length === 0
                text: qsTr("Dismiss")
                onClicked: root.dismissedBulkKey = root.bulkKey
            }
            Label {
                objectName: "modHealthNotice"
                Layout.fillWidth: true
                visible: !root.showOperation && !root.showBootstrap && !root.hasBulkResult && !root.library.error && root.library.warnings.length === 0 && root.checkMessage !== ""
                text: root.checkMessage
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                id: errorLabel
                objectName: "errorLabel"
                Layout.fillWidth: true
                visible: !root.showOperation && root.library.error !== ""
                text: Theme.friendlyError(root.library.error)
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            ScrollView {
                id: warningsView
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(warnings.implicitHeight + 4, 108)
                visible: root.library.warnings.length > 0
                contentWidth: availableWidth
                clip: true
                Label {
                    id: warnings
                    objectName: "warningLabel"
                    width: parent.width
                    text: root.library.warnings.map(warning => Theme.friendlyError(warning)).join("\n\n")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.pointSize: Theme.small
                    color: Theme.secondary
                }
            }
            RowLayout {
                id: operationRow
                Layout.fillWidth: true
                visible: !root.occupied && root.showOperation
                BusyIndicator {
                    objectName: "operationIndicator"
                    running: root.library.busy
                    visible: running
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 22
                }
                Label {
                    objectName: "operationStatus"
                    Layout.fillWidth: true
                    text: root.library.recoveryNeedsAttention ? Theme.recoveryText(root.library.error) : root.healthyHandoff ? qsTr("Close Deadlock to change your mods.") : root.library.externalGameinfoChange ? root.library.externalGameinfoMessage : root.library.outcomeUnknown ? qsTr("Check whether the previous change finished before editing.") : Theme.friendlyError(root.library.error || root.library.status)
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                Button {
                    objectName: "cancelPreviewButton"
                    visible: root.library.previewBusy
                    text: qsTr("Cancel review")
                    onClicked: root.library.cancelPreview()
                }
                Button {
                    objectName: "checkOperationButton"
                    visible: root.library.outcomeUnknown
                    enabled: !root.library.busy && !root.library.loading && root.library.bootstrapState !== "invalid"
                    text: ["pending", "creating", "confirming"].includes(root.library.bootstrapState) ? qsTr("Check creation") : qsTr("Check status")
                    onClicked: if (root.library.reconcileOperation())
                        root.forceActiveFocus()
                }
                Button {
                    objectName: "importThoseModsButton"
                    visible: (root.library.externalGameinfoChange || root.library.error.startsWith("unknownMount: Unrecognized addon mount: citadel/addons.")) && !root.library.recoveryRequired && !root.library.outcomeUnknown
                    enabled: root.library.canScanInstalledMods
                    text: qsTr("Import those mods")
                    onClicked: root.installedRequested()
                }
                Button {
                    objectName: "applyAgainButton"
                    visible: (root.library.externalGameinfoChange || root.library.error.startsWith("unknownMount: Unrecognized addon mount: citadel/addons.")) && !root.library.recoveryRequired && !root.library.outcomeUnknown
                    enabled: root.library.canPreviewApply
                    highlighted: true
                    text: qsTr("Apply again")
                    onClicked: root.applyRequested()
                }
                Button {
                    objectName: "recoverButton"
                    visible: (root.library.recoveryNeedsAttention || root.library.outcomeUnknown) && !["pending", "creating", "confirming", "invalid"].includes(root.library.bootstrapState)
                    enabled: root.library.canRecover
                    highlighted: true
                    text: qsTr("Recover")
                    onClicked: root.library.recover()
                }
            }
            ColumnLayout {
                id: bootstrapColumn
                Layout.fillWidth: true
                visible: root.showBootstrap && !root.showOperation && root.library.error === "" && root.library.warnings.length === 0
                spacing: 6
                ScrollView {
                    Layout.fillWidth: true
                    Layout.preferredHeight: Math.min(bootstrapDetails.implicitHeight + 4, 108)
                    contentWidth: availableWidth
                    clip: true
                    Label {
                        id: bootstrapDetails
                        objectName: "bootstrapReason"
                        width: parent.width
                        text: root.library.bootstrapReason
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.secondary
                        font.pointSize: Theme.small
                    }
                }
                Button {
                    objectName: "createEmptyLibraryButton"
                    visible: root.library.sourcePath === "" && root.library.bootstrapState === "absent"
                    enabled: root.library.canCreateLibrary
                    highlighted: true
                    text: qsTr("Create library")
                    onClicked: if (root.library.createLibrary())
                        root.forceActiveFocus()
                }
            }
        }
    }
}
