pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
Dialog {
    id: root
    required property LibraryModel library
    signal importReviewRequested()
    function phaseLabel(phase) {
        switch (phase) {
        case "enumerating": return qsTr("Scanning files")
        case "inspecting": return qsTr("Checking packages")
        case "hashing": return qsTr("Reading files")
        case "copyingArchives": return qsTr("Preparing original archives")
        case "listingArchives": return qsTr("Reading archive contents")
        case "extractingArchives": return qsTr("Preparing archive packages")
        case "ready": return qsTr("Ready to review")
        case "cancelRequested": return qsTr("Cancelling…")
        case "cancelled": return qsTr("Cancelled")
        case "interrupted": return qsTr("Interrupted")
        case "failed": return qsTr("Failed")
        case "committing": return qsTr("Saving")
        case "succeeded": return qsTr("Saved")
        case "recoveryRequired": return qsTr("Recovery needed")
        default: return qsTr("Unknown phase")
        }
    }
    readonly property var historyRows: library.localImport.jobs.map(row => ({purpose: "import", status: row})).concat(library.options.jobs.map(row => ({purpose: "options", status: row})))
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(780, parent ? parent.width - 32 : 780)
    height: Math.min(620, parent ? parent.height - 32 : 620)
    title: qsTr("Import history")
    modal: false
    standardButtons: Dialog.Close
    Component.onCompleted: { open(); history.forceActiveFocus() }
    onClosed: library.options.hideHistory()
    ColumnLayout {
        anchors.fill: parent
        Label {
            Layout.fillWidth: true
            text: qsTr("Up to eight recent imports are kept here. Cancel or dismiss old ones to make room; this also works for imports whose mod or profile was removed.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label { text: qsTr("%1 of 8 slots used").arg(root.historyRows.length) }
        ListView {
            id: history
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            reuseItems: true
            activeFocusOnTab: true
            model: root.historyRows
            spacing: 12
            ScrollBar.vertical: ScrollBar {}
            Accessible.name: qsTr("Retained file preparation jobs")
            delegate: ColumnLayout {
                id: jobRow
                required property var modelData
                readonly property var status: modelData.status
                readonly property bool options: modelData.purpose === "options"
                readonly property bool terminal: ["cancelled", "interrupted", "failed", "succeeded", "recoveryRequired"].includes(status.phase)
                width: history.width
                Label {
                    Layout.fillWidth: true
                    text: qsTr("%1 · %2 · %3").arg(jobRow.options ? qsTr("Options attachment") : qsTr("Import")).arg(jobRow.status.profileNameAtStart || jobRow.status.profileId).arg(jobRow.status.jobId)
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Label {
                    Layout.fillWidth: true
                    text: qsTr("%1 · %2 files · %3 read").arg(root.phaseLabel(jobRow.status.phase)).arg(jobRow.status.filesDone || 0).arg(Theme.bytes(jobRow.status.bytesDone))
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.pointSize: Theme.small
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!jobRow.status.error || !!jobRow.status.cleanupError
                    text: jobRow.status.error?.message || jobRow.status.cleanupError?.message || ""
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                RowLayout {
                    Button {
                        text: qsTr("Review")
                        visible: jobRow.status.phase === "ready"
                        enabled: jobRow.options ? root.library.options.available : !root.library.localImport.busy
                        onClicked: {
                            if (jobRow.options) root.library.options.showJob(jobRow.status.jobId)
                            else { root.library.localImport.showJob(jobRow.status.jobId); root.library.options.hideHistory(); root.importReviewRequested() }
                        }
                    }
                    Button {
                        text: qsTr("Cancel preparation")
                        visible: !!jobRow.status.canCancel
                        enabled: jobRow.options ? root.library.options.canCancelJob(jobRow.status.jobId) : root.library.localImport.canCancelJob(jobRow.status.jobId)
                        onClicked: jobRow.options ? root.library.options.cancelJob(jobRow.status.jobId) : root.library.localImport.cancelJob(jobRow.status.jobId)
                    }
                    Button {
                        text: qsTr("Dismiss")
                        visible: jobRow.terminal
                        enabled: !root.library.writeBusy && !root.library.options.busy && !root.library.outcomeUnknown && !root.library.recoveryRequired
                        onClicked: jobRow.options ? root.library.options.dismiss(jobRow.status.jobId) : root.library.localImport.dismiss(jobRow.status.jobId)
                    }
                }
            }
        }
        Label { Layout.fillWidth: true; text: root.library.options.error || root.library.localImport.error; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere }
    }
}
