pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Deadlock.Native

Pane {
    id: root
    required property LocalImportController controller
    signal reviewRequested()
    readonly property bool pickerOpen: files.visible || folders.visible || artworkDialog.visible
    property string sourceTicket: ""
    function beginSourcePicker() {
        if (pickerOpen) return false
        sourceTicket = controller.startSourcePicker()
        return sourceTicket !== ""
    }
    function openFiles() { if (beginSourcePicker()) files.open() }
    function openFolder() { if (beginSourcePicker()) folders.open() }
    function chooseArtwork() { if (!pickerOpen) artworkDialog.open() }
    function clearArtwork() { artwork = "" }
    function rejectSourcePicker() {
        controller.cancelSourcePicker(sourceTicket)
        sourceTicket = ""
    }
    readonly property var currentJob: controller.jobs.find(job => job.jobId === controller.jobId) || ({})
    function phaseLabel(phase) {
        switch (phase) {
        case "enumerating": return qsTr("Scanning files…")
        case "inspecting": return qsTr("Checking packages…")
        case "hashing": return qsTr("Reading files…")
        case "copyingArchives": return qsTr("Preparing original archives")
        case "listingArchives": return qsTr("Reading archive contents")
        case "extractingArchives": return qsTr("Preparing archive packages")
        case "ready": return qsTr("Ready to review")
        case "cancelRequested": return qsTr("Cancelling…")
        case "cancelled": return qsTr("Cancelled")
        case "interrupted": return qsTr("Interrupted")
        case "failed": return qsTr("Import failed")
        case "committing": return qsTr("Adding to library…")
        case "succeeded": return qsTr("Added to library")
        case "recoveryRequired": return qsTr("Recovery needed")
        default: return ""
        }
    }
    property url artwork
    // Only shown while an import is in progress or needs attention; starting an
    // import lives in the library's "Add mods" menu. Finished jobs from earlier
    // sessions stay in Settings → Import history instead of piling up here.
    property string sessionJob: ""
    readonly property bool terminal: ["cancelled", "interrupted", "failed", "succeeded"].includes(controller.phase)
    Connections {
        target: root.controller
        function onPreparationStarted(jobId, contextId) { root.sessionJob = jobId }
    }
    property bool occupied: false
    readonly property bool hasNotice: controller.error !== ""
             || (controller.jobId !== "" && (!terminal || controller.jobId === sessionJob))
    visible: hasNotice && !occupied
    padding: Theme.spacing12
    leftPadding: Theme.spacing24
    rightPadding: Theme.spacing24
    background: Rectangle {
        color: Theme.surfaceLow
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.rule }
    }
    ColumnLayout {
        anchors.fill: parent
        RowLayout {
            Layout.fillWidth: true
            visible: false
            Button {
                objectName: "localFilesButton"
                text: qsTr("Import files…")
                enabled: root.controller.canPrepare
                onClicked: root.openFiles()
            }
            Button {
                objectName: "localFolderButton"
                text: qsTr("Import folder…")
                enabled: root.controller.canPrepare
                onClicked: root.openFolder()
            }
            Button {
                text: root.artwork.toString() ? qsTr("Change artwork…") : qsTr("Optional artwork…")
                enabled: root.controller.canPrepare
                onClicked: artworkDialog.open()
            }
            Button {
                visible: root.artwork.toString() !== ""
                text: qsTr("Clear artwork")
                enabled: root.controller.canPrepare
                onClicked: root.artwork = ""
            }
            Item { Layout.fillWidth: true }
            Label {
                text: qsTr("VPK, ZIP, RAR or 7z files")
                textFormat: Text.PlainText
                color: Theme.secondary
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
        }
        RowLayout {
            visible: root.controller.jobId !== "" || root.controller.error !== ""
            Layout.fillWidth: true
            BusyIndicator { running: root.controller.busy; visible: running; Layout.preferredWidth: 22; Layout.preferredHeight: 22 }
            Label {
                objectName: "localJobStatus"
                font.weight: Font.Medium
                text: Theme.friendlyError(root.controller.error) || (root.currentJob.cleanupError ? qsTr("Temporary files couldn't be removed.") : root.controller.phase === "succeeded" ? qsTr("Added to library. Turn it on when you’re ready.") : qsTr("Import into %2: %1").arg(root.phaseLabel(root.controller.phase)).arg(root.controller.destinationName))
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
            Button { objectName: "localReviewButton"; highlighted: true; text: qsTr("Review import"); visible: root.controller.phase === "ready"; onClicked: root.reviewRequested() }
            Button { objectName: "localCancelButton"; text: root.controller.phase === "ready" ? qsTr("Discard review") : qsTr("Cancel import"); visible: root.controller.canCancel || root.controller.busy; enabled: root.controller.canCancel; onClicked: root.controller.cancel() }
            Button {
                objectName: "localDismissButton"
                text: qsTr("Dismiss")
                flat: true
                visible: ["cancelled", "interrupted", "failed", "succeeded", "recoveryRequired"].includes(root.controller.phase)
                enabled: root.controller.canDismiss
                onClicked: {
                    if (root.controller.dismiss(root.controller.jobId)) root.sessionJob = ""
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.controller.busy && root.controller.phase !== "committing" && root.currentJob.filesDone != null && root.currentJob.bytesDone != null
            text: {
                const job = root.currentJob
                const files = Number(job.filesDone) === 1 ? qsTr("1 file") : qsTr("%1 files").arg(Number(job.filesDone).toLocaleString(Qt.locale(), "f", 0))
                const read = job.bytesTotal == null ? Theme.bytes(job.bytesDone)
                           : qsTr("%1 of %2").arg(Theme.bytes(job.bytesDone)).arg(Theme.bytes(job.bytesTotal))
                return qsTr("%1 checked · %2 read").arg(files).arg(read)
            }
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Label {
            objectName: "localError"
            Layout.fillWidth: true
            visible: false
            text: Theme.friendlyError(root.controller.error)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.text
        }
        Label {
            Layout.fillWidth: true
            visible: false
            text: qsTr("Added, switched off. Turn it on in the library when you're ready.")
            color: Theme.secondary
            textFormat: Text.PlainText
        }
        Label {
            objectName: "localCleanupError"
            Layout.fillWidth: true
            visible: false
            text: qsTr("Temporary files couldn't be removed: %1").arg((root.currentJob.cleanupError || {}).message || "")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.text
        }
    }
    FileDialog {
        id: files
        title: qsTr("Choose local VPK files or archives")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("VPK files and archives (*.[vV][pP][kK] *.[zZ][iI][pP] *.[rR][aA][rR] *.7[zZ])"), qsTr("All files (*)")]
        onAccepted: {
            root.controller.preparePicked(root.sourceTicket, selectedFiles, root.artwork)
            root.sourceTicket = ""
        }
        onRejected: root.rejectSourcePicker()
    }
    FolderDialog {
        id: folders
        title: qsTr("Choose a local mod folder")
        onAccepted: {
            root.controller.preparePicked(root.sourceTicket, [selectedFolder], root.artwork)
            root.sourceTicket = ""
        }
        onRejected: root.rejectSourcePicker()
    }
    FileDialog {
        id: artworkDialog
        title: qsTr("Choose optional PNG or JPEG artwork")
        nameFilters: [qsTr("PNG and JPEG images (*.png *.jpg *.jpeg)")]
        onAccepted: root.artwork = selectedFile
    }
}
