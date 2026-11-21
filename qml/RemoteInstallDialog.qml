pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    required property LibraryModel library
    readonly property RemoteController controller: library.remote
    function uploadDate(value) {
        if (value === null || value === undefined || value === "") return qsTr("upload date unknown")
        const date = new Date(Number(value) * 1000)
        return isNaN(date.getTime()) ? qsTr("upload date unknown") : Qt.formatDate(date, Qt.DefaultLocaleShortDate)
    }
    readonly property string targetName: library.modDetails(controller.replacementModId).title || controller.replacementTarget.metadata?.name || controller.replacementModId
    readonly property string targetProfile: {
        if (!controller.replacementProfileId || controller.replacementProfileId === library.activeProfileId) return ""
        const profile = library.profiles.find(p => p.id === controller.replacementProfileId)
        return profile ? profile.name : controller.replacementProfileId
    }
    property bool showingDownloads: controller.downloadsRequested || controller.candidates.length > 0 || (!controller.hasResolution && !controller.replacementActive)
    readonly property bool selectingFiles: controller.continuingDownload === "" && (!controller.replacementActive || controller.replacementFileSelection)
    signal hidden()
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(920, parent ? parent.width - 32 : 920)
    height: Math.min(720, parent ? parent.height - 32 : 720)
    title: controller.downloadsRequested ? qsTr("Downloads") : controller.continuingDownload !== "" ? qsTr("Add to library") : controller.replacementActive ? (controller.replacementAction === "update" ? qsTr("Update %1").arg(root.targetName)
        : controller.replacementAction === "reinstall" ? qsTr("Reinstall %1").arg(root.targetName)
        : qsTr("Files for %1").arg(root.targetName)) + (root.targetProfile ? qsTr(" · %1").arg(root.targetProfile) : "")
        : controller.candidates.length > 0 ? qsTr("Choose a variant") : controller.hasResolution ? qsTr("Select files to download") : qsTr("Downloads")
    header: Label {
        text: root.title
        textFormat: Text.PlainText
        elide: Text.ElideRight
        font.pointSize: Theme.title
        font.weight: Font.DemiBold
        padding: 20
        bottomPadding: 8
    }
    modal: true
    focus: true
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: open()
    onClosed: { controller.close(); hidden() }
    ColumnLayout {
        anchors.fill: parent
        // "Ready" says nothing; only show the line while something is happening.
        ColumnLayout {
            Layout.fillWidth: true; Layout.fillHeight: true
            visible: root.library.batchUpdateReview
            Label { Layout.fillWidth: true; text: qsTr("Update these mods?"); font.weight: Font.DemiBold }
            ListView {
                Layout.fillWidth: true; Layout.fillHeight: true; clip: true
                model: root.library.batchUpdateTargets
                delegate: Label { required property var modelData; width: ListView.view.width; text: modelData.name || modelData.id; textFormat: Text.PlainText; elide: Text.ElideRight; padding: 8 }
                ScrollBar.vertical: ScrollBar {}
            }
            Button { objectName: "confirmBatchUpdates"; text: qsTr("Update all"); highlighted: true; onClicked: root.library.confirmBatchUpdates() }
        }
        Label { Layout.fillWidth: true; visible: !root.library.batchUpdateReview && !root.showingDownloads && !["idle", "Downloads ready.", "Choose a file to install."].includes(root.controller.phase); text: root.controller.phase; textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.secondary }
        Label { Layout.fillWidth: true; visible: !root.library.batchUpdateReview && !root.showingDownloads && root.controller.error !== ""; text: Theme.downloadError(root.controller.error); textFormat: Text.PlainText; wrapMode: Text.Wrap }
        Label {
            Layout.fillWidth: true
            visible: !root.library.batchUpdateReview && !root.showingDownloads && root.controller.linkedFileId !== ""
            text: qsTr("Opened from a GameBanana link")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        StackLayout {
            visible: !root.library.batchUpdateReview
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: root.showingDownloads ? 1 : 0
            ColumnLayout {
                RowLayout {
                    visible: root.controller.replacementAction === "link" && root.controller.replacementTargetCurrent && !root.controller.hasResolution
                    ComboBox { id: linkType; model: ["mod", "sound", "wip"]; Accessible.name: qsTr("Submission type") }
                    TextField { id: linkId; Layout.fillWidth: true; placeholderText: qsTr("GameBanana submission ID"); validator: RegularExpressionValidator { regularExpression: /[1-9][0-9]{0,18}/ } }
                    Button {
                        text: qsTr("Choose submission")
                        highlighted: true
                        enabled: linkId.acceptableInput && root.controller.canResolve
                        onClicked: root.controller.resolve({provider: "gamebanana", submissionType: linkType.currentText, submissionId: linkId.text})
                    }
                }
                Label {
                    objectName: "replacementVersionState"
                    Layout.fillWidth: true
                    text: root.controller.continuingDownload !== "" ? qsTr("Confirm this download's file, then add it to your library.") : root.controller.replacementFileSelection ? qsTr("Version unknown, update to the latest file") : root.controller.replacementActive
                          ? qsTr("Choose a file from this check. File details do not establish whether it is newer or matches installed files.")
                          : qsTr("Select the files you want. They will appear together in Library.")
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: root.controller.replacementActive && root.controller.replacementTargetCurrent && root.controller.savedFileObservation !== ""
                    text: root.controller.savedFileObservation === "archived" ? qsTr("The saved file is listed as archived.")
                        : root.controller.savedFileObservation === "listed" ? qsTr("The saved file is listed in this check.")
                        : qsTr("The saved file was not listed in this completed check.")
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: root.controller.replacementActive && (root.controller.replacementSkip.state === "unavailable" || root.controller.skipPending)
                    text: root.controller.skipPending ? qsTr("The skipped-file choice is not yet confirmed. You can still choose Force update or Reinstall.")
                        : qsTr("Skipped-file choices could not be checked. You can still choose Force update or Reinstall.")
                    wrapMode: Text.Wrap; color: Theme.secondary
                }
                Button {
                    visible: root.controller.replacementActive && !root.controller.replacementTargetCurrent && !root.controller.replacementObservationPending && (!!root.controller.replacementTarget.modId || root.controller.error !== "")
                    text: qsTr("Check download details again")
                    enabled: root.controller.replacementAvailable && (!root.controller.replacementJob || root.controller.job.purpose !== "update" || !root.controller.skipPending)
                    onClicked: root.controller.refreshReplacementObservation()
                }
                CheckBox {
                    objectName: "selectAllDownloads"
                    visible: root.selectingFiles
                    text: qsTr("Select all")
                    checked: root.controller.files.length > 0 && root.controller.selectedFileIds.length === root.controller.files.length
                    onToggled: root.controller.pickAllFiles(checked)
                }
                ListView {
                    id: files
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    reuseItems: false
                    spacing: 12
                    model: root.controller.files
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Rectangle {
                        id: file
                        required property var modelData
                        width: files.width
                        implicitHeight: fileColumn.implicitHeight + 24
                        radius: Theme.radius
                        color: Theme.surfaceLow
                        border.color: file.needsAcknowledgement ? Theme.rule : Qt.alpha(Theme.success, 0.5)
                        property bool detailsOpen: false
                        readonly property bool latest: modelData.list !== "archived" && modelData.archiveFlag === false
                        readonly property var key: modelData.fileKey || ({})
                        readonly property var savedSkip: root.controller.replacementSkip.entry || ({})
                        readonly property bool skipped: root.controller.replacementSkip.state === "present" && savedSkip.fileId === key.fileId && savedSkip.candidateFingerprint === modelData.candidateFingerprint
                        readonly property bool skipStored: (root.controller.replacementSkip.state === "present" || root.controller.replacementSkip.state === "stale") && savedSkip.fileId === key.fileId && savedSkip.candidateFingerprint === modelData.candidateFingerprint
                        readonly property bool savedRoot: key.fileId === (root.controller.replacementTarget.receipt || {}).rootFileId
                        readonly property bool needsAcknowledgement: modelData.list === "archived" || modelData.archiveFlag === true || modelData.archiveFlag === null || modelData.archiveFlag === undefined
                        ColumnLayout {
                        id: fileColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 12
                        spacing: 6
                        RowLayout {
                            Layout.fillWidth: true
                            CheckBox {
                                visible: root.selectingFiles
                                Accessible.name: file.modelData.originalName || qsTr("Download file")
                                checked: root.controller.selectedFileIds.includes(file.key.fileId)
                                enabled: !file.needsAcknowledgement || acknowledgement.checked
                                onToggled: root.controller.pickFile(file.key.fileId, checked, acknowledgement.checked)
                            }
                            Label { Layout.fillWidth: true; text: file.modelData.originalName || ""; textFormat: Text.PlainText; elide: Text.ElideMiddle; font.weight: Font.DemiBold }
                            StatusPill {
                                text: file.latest ? qsTr("Latest") : file.modelData.list === "archived" || file.modelData.archiveFlag === true ? qsTr("Older version") : qsTr("Status unknown")
                                tone: file.latest ? "success" : file.modelData.list === "archived" || file.modelData.archiveFlag === true ? "muted" : "warning"
                            }
                        }
                        Label {
                            objectName: "remoteFileDescription"
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: file.modelData.description || ""
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            maximumLineCount: 3
                            elide: Text.ElideRight
                            color: Theme.secondary
                        }
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("%1 · %2").arg(file.modelData.declaredSize === null ? qsTr("Size unknown") : Theme.bytes(file.modelData.declaredSize)).arg(root.uploadDate(file.modelData.createdAt))
                            textFormat: Text.PlainText; color: Theme.secondary
                        }
                        ToolButton {
                            text: file.detailsOpen ? qsTr("Hide details") : qsTr("Details")
                            onClicked: file.detailsOpen = !file.detailsOpen
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: file.detailsOpen
                            text: qsTr("File ID %1 · MD5 %2 · %3 · %4")
                                .arg(file.key.fileId || "")
                                .arg(file.modelData.md5 || qsTr("not supplied"))
                                .arg(file.modelData.list === "archived" ? qsTr("archived list") : qsTr("current list"))
                                .arg(file.modelData.archiveFlag === null || file.modelData.archiveFlag === undefined ? qsTr("archive flag unknown") : file.modelData.archiveFlag ? qsTr("marked archived") : qsTr("not marked archived"))
                            textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.faint; font.pointSize: Theme.small
                        }
                        CheckBox {
                            id: acknowledgement
                            readonly property string consentIdentity: JSON.stringify([root.controller.resolutionIdentity, file.key.fileId, file.modelData.candidateFingerprint])
                            onConsentIdentityChanged: checked = false
                            visible: file.needsAcknowledgement
                            onToggled: if (!checked && root.selectingFiles) root.controller.pickFile(file.key.fileId, false)
                            text: qsTr("Install this older or unconfirmed version anyway")
                            contentItem: Label { text: acknowledgement.text; leftPadding: 28; textFormat: Text.PlainText; wrapMode: Text.Wrap }
                        }
                        Flow {
                            Layout.fillWidth: true; spacing: 6
                            Button {
                                visible: root.controller.replacementActive && !root.selectingFiles && root.controller.replacementAction !== "resume" && root.controller.replacementAction !== "reinstall" && (root.controller.replacementAction === "link" || (!file.skipped && root.controller.replacementSkip.state !== "unavailable" && !root.controller.skipPending))
                                highlighted: true
                                text: root.controller.replacementAction === "link" ? qsTr("Link and replace with this file") : qsTr("Update with this file")
                                enabled: root.controller.replacementAvailable && root.controller.replacementTargetCurrent && !root.controller.skipPending && !!root.controller.replacementSkip.state && (!file.needsAcknowledgement || acknowledgement.checked)
                                onClicked: if (root.controller.startReplacement(file.key.fileId, file.modelData.candidateFingerprint, root.controller.replacementAction === "link" ? "link" : "update", acknowledgement.checked)) root.showingDownloads = true
                            }
                            Button {
                                visible: root.controller.replacementActive && !root.selectingFiles && root.controller.replacementAction !== "link" && root.controller.replacementAction !== "resume" && root.controller.replacementAction !== "reinstall" && (file.skipped || root.controller.replacementSkip.state === "unavailable" || root.controller.skipPending)
                                text: qsTr("Force update with this file")
                                enabled: root.controller.replacementAvailable && root.controller.replacementTargetCurrent && !!root.controller.replacementSkip.state && (!file.needsAcknowledgement || acknowledgement.checked)
                                onClicked: if (root.controller.startReplacement(file.key.fileId, file.modelData.candidateFingerprint, "force", acknowledgement.checked)) root.showingDownloads = true
                            }
                            Button {
                                visible: root.controller.replacementAction === "reinstall" && file.savedRoot
                                text: qsTr("Reinstall this file")
                                enabled: root.controller.replacementAvailable && root.controller.replacementTargetCurrent && !!root.controller.replacementSkip.state && (!file.needsAcknowledgement || acknowledgement.checked)
                                onClicked: if (root.controller.startReplacement(file.key.fileId, file.modelData.candidateFingerprint, "reinstall", acknowledgement.checked)) root.showingDownloads = true
                            }
                            ToolButton {
                                visible: root.controller.replacementActive && !root.selectingFiles && root.controller.replacementAction !== "link" && root.controller.replacementAction !== "resume" && root.controller.replacementSkip.state !== "unavailable"
                                enabled: root.controller.replacementTargetCurrent && !root.controller.skipPending && !!root.controller.replacementSkip.state
                                text: qsTr("More…")
                                onClicked: skipMenu.open()
                                Menu {
                                    id: skipMenu
                                    MenuItem { visible: !file.skipped; text: qsTr("Skip this file"); onTriggered: root.controller.changeSkip(file.key.fileId, file.modelData.candidateFingerprint, false) }
                                    MenuItem { visible: file.skipStored; text: qsTr("Unskip this file"); onTriggered: root.controller.changeSkip(file.key.fileId, file.modelData.candidateFingerprint, true) }
                                }
                            }
                            Button {
                                visible: root.controller.currentJobId !== "" && root.controller.job.state === "downloaded" && (!root.controller.replacementJob || root.controller.replacementAction === "resume") && file.key.fileId === ((root.controller.job.cohort || {}).members || [])[0]?.fileId
                                text: qsTr("Add to library")
                                enabled: !file.needsAcknowledgement || acknowledgement.checked
                                onClicked: if (root.controller.bindConsent(root.controller.currentJobId, file.key.fileId, file.modelData.candidateFingerprint, acknowledgement.checked)) { root.showingDownloads = true; root.controller.control(root.controller.currentJobId, "", "install") }
                            }
                        }
                        Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Theme.rule }
                    }
                        }
                }
                RowLayout {
                    Button { visible: !root.selectingFiles; text: qsTr("Previous"); enabled: root.controller.filePage > 0; onClicked: root.controller.loadFiles(root.controller.filePage - 1) }
                    Label { visible: !root.selectingFiles; text: qsTr("Page %1").arg(root.controller.filePage + 1) }
                    Button { visible: !root.selectingFiles; text: qsTr("Next"); enabled: root.controller.fileHasMore; onClicked: root.controller.loadFiles(root.controller.filePage + 1) }
                    Item { Layout.fillWidth: true }
                    Label { visible: root.selectingFiles; text: qsTr("Total: %1").arg(Theme.bytes(root.controller.selectedFileBytes)) }
                    Button {
                        objectName: "downloadSelectedFiles"
                        visible: root.selectingFiles
                        text: qsTr("Download selected")
                        highlighted: true
                        enabled: root.controller.selectedFileIds.length > 0
                        onClicked: root.controller.startSelectedFiles()
                    }
                }
            }
            DownloadHistory { library: root.library; onCloseRequested: root.close() }

        }
        RowLayout { Item { Layout.fillWidth: true } Button { objectName: "closeDownloadReview"; text: root.library.updatingCount > 0 ? qsTr("Cancel remaining") : qsTr("Close"); onClicked: { if (root.library.updatingCount > 0) root.library.cancelRemainingUpdates(); root.close() } } }
    }
}
