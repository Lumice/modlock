pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Deadlock.Native

Dialog {
    id: root
    required property OptionController controller
    readonly property bool preparing: section.currentIndex === 1
    readonly property bool reviewing: preparing ? !!controller.attachmentReview.token : !!controller.selectionReview.token
    readonly property var bindingChoices: [{id: "", optionName: qsTr("No file selected")}].concat(controller.candidates.filter(row => row.available))
    function candidateLabel(row) {
        if (!row || !row.id) return row ? row.optionName : ""
        const source = row.archiveRelativePath ? qsTr("%1 / %2").arg(row.archiveRelativePath).arg(row.relativePath) : row.relativePath
        return qsTr("%1 · source %2 · %3").arg(row.optionName).arg(row.rootOrdinal + 1).arg(source)
    }
    function stateLabel(state) {
        switch (state) {
        case "ready": return qsTr("Retained")
        case "needsPreparation": return qsTr("Original file needs to be checked")
        case "needsSource": return qsTr("Choose the original file")
        default: return qsTr("Original file is unavailable; import another copy")
        }
    }
    function phaseLabel(phase) {
        switch (phase) {
        case "enumerating": return qsTr("Scanning files")
        case "inspecting": return qsTr("Checking packages")
        case "hashing": return qsTr("Reading files")
        case "copyingArchives": return qsTr("Preparing original archives")
        case "listingArchives": return qsTr("Reading archive contents")
        case "extractingArchives": return qsTr("Preparing archive packages")
        case "ready": return qsTr("Ready to review files")
        case "cancelRequested": return qsTr("Cancelling…")
        case "committing": return qsTr("Saving files")
        case "succeeded": return qsTr("Files saved")
        case "cancelled": return qsTr("Cancelled")
        case "interrupted": return qsTr("Interrupted; prepare again")
        case "failed": return qsTr("Preparation failed")
        case "recoveryRequired": return qsTr("Check status or recover")
        default: return ""
        }
    }
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(820, parent ? parent.width - 32 : 820)
    height: Math.min(700, parent ? parent.height - 32 : 700)
    title: qsTr("Files and options")
    // Save reviews and then commits in one step; the review stays visible on
    // request (Show changes) or when saving needs attention.
    property bool autoSave: false
    Connections {
        target: root.controller
        function onChanged() {
            if (!root.autoSave) return
            if (root.controller.error) { root.autoSave = false; return }
            if (root.controller.canSave) { root.autoSave = false; root.controller.commitSelection(); return }
            // A finished review that cannot be committed (e.g. nothing changed) ends this Save.
            if (root.controller.selectionReview.token && !root.controller.busy) root.autoSave = false
        }
    }
    modal: false
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: { open(); choices.forceActiveFocus() }
    onClosed: controller.hide()
    ColumnLayout {
        anchors.fill: parent
        spacing: 10
        Label {
            Layout.fillWidth: true
            text: qsTr("%1 · %2").arg(root.controller.profileName).arg(root.controller.targetName)
            textFormat: Text.PlainText
            elide: Text.ElideRight
            font.weight: Font.Medium
        }
        Label {
            Layout.fillWidth: true
            text: root.preparing ? qsTr("Add the original files for missing choices. Your selections and installed fonts stay the same.") : qsTr("Pick which files this mod uses in this profile. Apply changes puts them in the game.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !root.reviewing
            text: qsTr("%1 of %2 files selected · %3 stored").arg(root.controller.selectedIds.length).arg(root.controller.rows.length).arg(Number(root.controller.totals.uniqueCopyBytes || 0) < 1048576 ? qsTr("%1 KB").arg(Math.ceil(Number(root.controller.totals.uniqueCopyBytes || 0) / 1024)) : qsTr("%1 MB").arg((Number(root.controller.totals.uniqueCopyBytes || 0) / 1048576).toFixed(1)))
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.pointSize: Theme.small
        }
        TabBar {
            id: section
            Layout.fillWidth: true
            TabButton { text: qsTr("Files") }
            TabButton { text: qsTr("Add missing files") }
        }
        ListView {
            id: choices
            visible: !root.reviewing
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            reuseItems: true
            activeFocusOnTab: true
            model: root.controller.rows
            spacing: 10
            ScrollBar.vertical: ScrollBar {}
            Accessible.name: qsTr("Saved file options")
            delegate: ColumnLayout {
                id: option
                required property var modelData
                width: choices.width
                spacing: 4
                CheckBox {
                    id: optionChoice
                    visible: !root.preparing
                    Layout.fillWidth: true
                    text: option.modelData.optionName
                    contentItem: Label {
                        text: option.modelData.optionName
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                        leftPadding: optionChoice.indicator.width + optionChoice.spacing
                        verticalAlignment: Text.AlignVCenter
                    }
                    checked: root.controller.selectedIds.includes(option.modelData.id)
                    enabled: root.controller.editable && option.modelData.state === "ready"
                    onToggled: { root.controller.setSelected(option.modelData.id, checked); checked = Qt.binding(() => root.controller.selectedIds.includes(option.modelData.id)) }
                }
                Label {
                    Layout.fillWidth: true
                    visible: root.preparing
                    text: option.modelData.optionName
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    font.weight: Font.Medium
                }
                Label {
                    Layout.fillWidth: true
                    text: qsTr("%1 · %2 companions").arg(root.stateLabel(option.modelData.state)).arg(option.modelData.companionCount)
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.secondary
                }
                Label {
                    Layout.fillWidth: true
                    visible: !!option.modelData.reason
                    text: option.modelData.reason || ""
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    font.pointSize: Theme.small
                }
                RowLayout {
                    visible: root.preparing && option.modelData.state !== "ready"
                    Layout.fillWidth: true
                    Button {
                        visible: option.modelData.state === "needsPreparation"
                        text: qsTr("Check saved archive…")
                        enabled: root.controller.canPrepare
                        onClicked: root.controller.prepareArchive(option.modelData.archiveId)
                    }
                    Button {
                        visible: option.modelData.state === "needsSource" || option.modelData.state === "needsPreparation"
                        text: qsTr("Choose source…")
                        enabled: root.controller.canPrepare
                        onClicked: sourceMenu.popup()
                        Menu {
                            id: sourceMenu
                            MenuItem { text: qsTr("Choose files…"); onTriggered: sourceFiles.open() }
                            MenuItem { text: qsTr("Choose folder…"); onTriggered: sourceFolder.open() }
                        }
                    }
                }
                ComboBox {
                    id: binding
                    Layout.fillWidth: true
                    visible: root.preparing && option.modelData.state !== "ready" && root.controller.job.phase === "ready"
                    enabled: root.controller.canBind
                    model: root.bindingChoices
                    textRole: "optionName"
                    valueRole: "id"
                    currentIndex: {
                        const bound = root.controller.bindings.find(row => row.optionId === option.modelData.id)
                        return bound ? root.bindingChoices.findIndex(row => row.id === bound.candidateId) : 0
                    }
                    Accessible.name: qsTr("Prepared source for %1").arg(option.modelData.optionName)
                    Accessible.description: root.candidateLabel(root.bindingChoices[currentIndex])
                    ToolTip.visible: hovered || activeFocus
                    ToolTip.text: Accessible.description
                    contentItem: Label {
                        text: root.candidateLabel(root.bindingChoices[binding.currentIndex])
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        verticalAlignment: Text.AlignVCenter
                        leftPadding: binding.leftPadding
                        rightPadding: binding.indicator.width + binding.spacing
                    }
                    delegate: ItemDelegate {
                        id: prepared
                        required property var modelData
                        required property int index
                        width: binding.width
                        highlighted: binding.highlightedIndex === index
                        contentItem: Label { text: root.candidateLabel(prepared.modelData); textFormat: Text.PlainText; elide: Text.ElideRight }
                    }
                    onActivated: root.controller.setBinding(option.modelData.id, currentValue)
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.preparing && !!root.controller.job.phase && !root.reviewing
            text: qsTr("%1 · %2 files · %3 read%4").arg(root.phaseLabel(root.controller.job.phase || "")).arg(root.controller.job.filesDone || 0).arg(Theme.bytes(root.controller.job.bytesDone)).arg(root.controller.job.bytesTotal == null ? "" : qsTr(" of %1").arg(Theme.bytes(root.controller.job.bytesTotal)))
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        ProgressBar {
            Layout.fillWidth: true
            visible: root.preparing && !!root.controller.job.canCancel && root.controller.job.phase !== "ready"
            indeterminate: root.controller.job.bytesTotal == null || root.controller.job.bytesTotal === 0
            from: 0
            to: root.controller.job.bytesTotal || 1
            value: root.controller.job.bytesDone || 0
        }
        Label {
            Layout.fillWidth: true
            visible: root.preparing && root.controller.job.phase === "ready" && !root.controller.canBind && !root.controller.busy
            text: qsTr("This preparation belongs to an earlier setup. Cancel it and prepare again before attaching bytes.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        RowLayout {
            Layout.fillWidth: true
            visible: root.preparing && !root.reviewing
            Button { text: qsTr("Preparation history…"); onClicked: root.controller.showHistory() }
            Button { text: qsTr("Cancel preparation"); visible: root.preparing && !!root.controller.job.canCancel; enabled: root.controller.canCancelJob(root.controller.job.jobId); onClicked: root.controller.cancelJob(root.controller.job.jobId) }
        }
        Label {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: root.preparing && !!root.controller.attachmentReview.token
            text: {
                const sm = root.controller.attachmentReview.summary || {}
                return qsTr("Add %1 more file choices to this mod (%2). What's switched on stays the same.")
                    .arg(sm.attachedOptionCount || 0).arg(Theme.bytes(Number(sm.newPayloadBytes || 0) + Number(sm.newArchiveBytes || 0)))
            }
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: root.preparing && !!root.controller.attachmentReview.token
            text: qsTr("After attachment: %1 retained families · %2 total retained bytes").arg(root.controller.attachmentReview.summary?.resultingTotals?.retainedFamilies || 0).arg(root.controller.attachmentReview.summary?.resultingTotals?.uniqueCopyBytes || 0)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        ProfileEditReview { Layout.fillWidth: true; Layout.fillHeight: true; visible: !root.preparing && !!root.controller.selectionReview.token; review: root.controller.selectionReview }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(80, errorText.implicitHeight)
            visible: !!root.controller.error
            contentWidth: availableWidth
            Label { id: errorText; width: parent.width; text: root.controller.error; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere }
        }
    }
    footer: Frame {
        padding: 16
        background: null
        implicitHeight: footerButtons.implicitHeight + topPadding + bottomPadding
        RowLayout {
            id: footerButtons
            anchors.fill: parent
            Button { text: qsTr("Close"); onClicked: root.close() }
            Button { text: qsTr("Undo edits"); flat: true; enabled: !root.controller.busy; onClicked: { root.autoSave = false; root.controller.cancel() } }
            Item { Layout.fillWidth: true }
            Button { objectName: "reviewSelectionButton"; flat: true; visible: !root.preparing && !root.reviewing; text: qsTr("Show changes"); enabled: root.controller.editable && root.controller.selectedIds.length > 0; onClicked: { root.autoSave = false; root.controller.previewSelection() } }
            Button { text: qsTr("Back to choices"); visible: root.reviewing; enabled: !root.controller.busy; onClicked: { root.autoSave = false; root.preparing ? root.controller.discardAttachmentReview() : root.controller.discardSelectionReview() } }
            Button { text: qsTr("Review files"); highlighted: true; visible: root.preparing && !root.reviewing && root.controller.job.phase === "ready"; enabled: root.controller.canBind && root.controller.bindings.length > 0; onClicked: root.controller.reviewBindings(root.controller.bindings) }
            Button { text: qsTr("Add files"); highlighted: true; visible: root.preparing && !!root.controller.attachmentReview.token; enabled: root.controller.canAttach; onClicked: root.controller.attach() }
            Button {
                objectName: "saveSelectionButton"
                visible: !root.preparing
                highlighted: true
                text: qsTr("Save")
                enabled: root.controller.canSave || (!root.reviewing && root.controller.editable && root.controller.selectedIds.length > 0 && !root.controller.busy)
                onClicked: {
                    if (root.controller.canSave) root.controller.commitSelection()
                    else { root.autoSave = true; if (!root.controller.previewSelection()) root.autoSave = false }
                }
            }
        }
    }
    FileDialog { id: sourceFiles; fileMode: FileDialog.OpenFiles; title: qsTr("Choose original VPK files or archives"); onAccepted: root.controller.prepareLocal(selectedFiles) }
    FolderDialog { id: sourceFolder; title: qsTr("Choose original content folder"); onAccepted: root.controller.prepareLocal([selectedFolder]) }
}
