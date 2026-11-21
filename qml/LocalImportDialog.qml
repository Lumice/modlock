pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "FontPresentation.js" as FontPresentation

Dialog {
    id: root
    required property LocalImportController controller
    property bool previewAttempted: false
    property var detailRows: null
    property string detailTitle
    function exclusionLabel(reason) {
        switch (reason) {
        case "archiveUnsupported": return qsTr("Unsupported archive format")
        case "fontUnsupported": return qsTr("Font import is pending")
        case "nestedArchiveNotExpanded": return qsTr("Nested archives are not expanded")
        case "linkOrSpecialEntry": return qsTr("Links and special files are excluded")
        case "notVpk": return qsTr("Not a VPK package")
        case "requiredCompanionMissing": return qsTr("Required companion is missing")
        default: return reason || ""
        }
    }
    signal hidden()
    signal retryRequested()
    parent: Overlay.overlay
    width: Math.min(900, parent ? parent.width - 32 : 900)
    height: Math.min(700, parent ? parent.height - 32 : 700)
    anchors.centerIn: parent
    title: qsTr("Add to library")
    modal: false
    closePolicy: Popup.CloseOnEscape
    Component.onCompleted: open()
    onClosed: hidden()
    ColumnLayout {
        anchors.fill: parent
        Label {
            Layout.fillWidth: true
            visible: (root.controller.review.fontCandidates || []).length > 0
            text: qsTr("This mod includes fonts. They're saved with the mod but not installed; you can install them afterwards.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Adding to %1. New mods start switched off.").arg(root.controller.destinationName)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(70, sourceRoots.implicitHeight)
            contentWidth: availableWidth
            Label {
                id: sourceRoots
                width: parent.width
                text: (root.controller.review.sources || []).map(source => source.path).join("\n")
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
        }
        SplitView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            orientation: Qt.Horizontal
            ListView {
                id: candidateList
                SplitView.fillWidth: true
                SplitView.minimumWidth: 260
                clip: true
                spacing: 8
                reuseItems: true
                model: (root.controller.review.candidates || []).concat(root.controller.review.fontCandidates || []).concat((root.controller.review.fontDiagnostics || []).filter(row => (row.origins || []).length === 0)).concat(root.controller.review.exclusions || []).concat(root.controller.review.warnings || [])
                ScrollBar.vertical: ScrollBar {}
                delegate: ColumnLayout {
                    id: candidate
                    required property var modelData
                    width: candidateList.width
                    readonly property bool fontRow: (modelData.id || "").length === 32
                    readonly property bool packageRow: modelData.available !== undefined && !fontRow
                    CheckBox {
                        objectName: "localCandidate"
                        Layout.fillWidth: true
                        visible: candidate.packageRow
                        text: candidate.modelData.optionName || ""
                        contentItem: Label {
                            text: candidate.modelData.optionName || ""
                            textFormat: Text.PlainText
                            leftPadding: 28
                            verticalAlignment: Text.AlignVCenter
                            wrapMode: Text.Wrap
                        }
                        enabled: candidate.modelData.available === true && !root.controller.busy
                        checked: root.controller.selectionDraft.includes(candidate.modelData.id || "")
                        onToggled: {
                            let selection = root.controller.selectionDraft.slice()
                            selection = selection.filter(id => id !== candidate.modelData.id)
                            if (checked) selection.push(candidate.modelData.id)
                            root.controller.selectionDraft = selection
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        text: candidate.fontRow ? qsTr("Font: %1\nFamily: %2\n%3").arg(candidate.modelData.targetName || "").arg(candidate.modelData.displayName || "").arg(candidate.modelData.unavailableReason || qsTr("Validated for retention; installation needs a separate review.")) : candidate.packageRow
                            ? (candidate.modelData.archiveRelativePath
                                ? qsTr("Archive: %1\nPackage: %2").arg(candidate.modelData.archiveRelativePath).arg(candidate.modelData.relativePath)
                                : candidate.modelData.relativePath) + (candidate.modelData.available ? "" : "\n" + candidate.modelData.unavailableReason)
                            : candidate.modelData.message ||
                                ((candidate.modelData.archiveRelativePath
                                    ? qsTr("%1 → %2").arg(candidate.modelData.archiveRelativePath).arg(candidate.modelData.relativePath)
                                    : candidate.modelData.relativePath) + " — " + root.exclusionLabel(candidate.modelData.reason))
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                        color: candidate.packageRow && !candidate.modelData.available ? Theme.text : Theme.secondary
                    }
                    Button {
                        visible: candidate.fontRow && (candidate.modelData.origins || []).length > 0
                        text: qsTr("Font provenance…")
                        onClicked: {
                            root.detailTitle = candidate.modelData.targetName || qsTr("Font provenance")
                            root.detailRows = candidate.modelData.origins.map(origin => ({
                                sourceName: origin.resourcePath || candidate.modelData.targetName || "",
                                fontOrigin: origin,
                                size: candidate.modelData.size || 0
                            }))
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        visible: candidate.packageRow && (candidate.modelData.members || []).length > 0
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("%1 package files").arg((candidate.modelData.members || []).length)
                            color: Theme.secondary
                        }
                        Button {
                            text: qsTr("File details…")
                            onClicked: {
                                root.detailTitle = candidate.modelData.optionName
                                root.detailRows = candidate.modelData.members
                            }
                        }
                    }
                }
            }
            ScrollView {
                SplitView.preferredWidth: 320
                SplitView.minimumWidth: 220
                contentWidth: availableWidth
                ColumnLayout {
                    width: parent.width
                    Repeater {
                        model: [{key:"name",label:qsTr("Name")},{key:"author",label:qsTr("Author")},{key:"category",label:qsTr("Category")},{key:"link",label:qsTr("Link")}]
                        ColumnLayout {
                            id: field
                            required property var modelData
                            Layout.fillWidth: true
                            Label { text: field.modelData.label }
                            TextField {
                                objectName: "localField_" + field.modelData.key
                                Layout.fillWidth: true
                                enabled: !root.controller.busy
                                text: root.controller.metadataDraft[field.modelData.key] || ""
                                Accessible.name: field.modelData.label
                                onTextEdited: {
                                    const draft = Object.assign({}, root.controller.metadataDraft)
                                    draft[field.modelData.key] = text
                                    root.controller.metadataDraft = draft
                                }
                            }
                        }
                    }
                    Label { text: qsTr("Description") }
                    TextArea {
                        objectName: "localDescription"
                        Layout.fillWidth: true
                        Layout.preferredHeight: 80
                        enabled: !root.controller.busy
                        text: root.controller.metadataDraft.description || ""
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        Accessible.name: qsTr("Description")
                        onTextChanged: {
                            if (activeFocus) {
                                const draft = Object.assign({}, root.controller.metadataDraft)
                                draft.description = text
                                root.controller.metadataDraft = draft
                            }
                        }
                    }
                    ModPreview {
                        objectName: "localArtworkPreview"
                        Layout.fillWidth: true
                        Layout.preferredHeight: 140
                        source: root.controller.review.artworkUrl || ""
                        active: root.visible
                        decodeSize: Qt.size(640, 400)
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            text: root.controller.review.token
                ? (root.controller.review.selectedPackageCount === 1 ? qsTr("1 selected package") : qsTr("%1 selected packages").arg(root.controller.review.selectedPackageCount))
                    + " · " + qsTr("%1 to copy").arg(Theme.bytes(root.controller.review.uniqueCopyBytes))
                : !root.controller.presentationReady ? qsTr("Preparing package details…")
                : !root.controller.draftCurrent ? qsTr("This review is no longer current. Close it and review the import again.")
                : root.controller.selectionDraft.length === 0 ? qsTr("Select at least one package to continue.")
                : root.previewAttempted && root.controller.canReview ? qsTr("Review is unavailable. Check the details below and retry.")
                : qsTr("Preparing the review or waiting for other work to finish…")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.controller.review.token && root.controller.review.inactiveOptionCount !== undefined
            text: qsTr("%1 other choices are kept for later; only the selected files are switched on.").arg(root.controller.review.inactiveOptionCount)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Button {
            visible: (root.controller.review.archives || []).length > 0
            text: qsTr("Retained originals (%1)…").arg((root.controller.review.archives || []).length)
            onClicked: {
                root.detailTitle = qsTr("Retained original archives")
                root.detailRows = root.controller.review.archives
            }
        }
        Label { Layout.fillWidth: true; visible: root.controller.error !== ""; text: root.controller.error; textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.text }
        RowLayout {
            Button { id: closeReview; objectName: "localHideReview"; text: qsTr("Close review"); onClicked: root.close() }
            Item { Layout.fillWidth: true }
            Button {
                objectName: "localRetryReview"
                text: qsTr("Retry review")
                visible: root.previewAttempted && !root.controller.review.token && root.controller.selectionDraft.length > 0
                enabled: root.controller.draftCurrent && root.controller.canReview
                onClicked: root.retryRequested()
            }
            Button { objectName: "localCommitButton"; text: qsTr("Add to library"); enabled: root.controller.canCommit; onClicked: { if (root.controller.commit()) root.close() } }
        }
    }
    Loader {
        active: root.detailRows !== null && root.visible
        sourceComponent: Component {
            Dialog {
                id: details
                property var closeAction: null
                parent: Overlay.overlay
                anchors.centerIn: parent
                width: Math.min(700, parent ? parent.width - 48 : 700)
                height: Math.min(480, parent ? parent.height - 48 : 480)
                title: root.detailTitle
                header: Label {
                    text: root.detailTitle
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    padding: 16
                    font.weight: Font.Medium
                }
                modal: true
                standardButtons: Dialog.Close
                Component.onCompleted: {
                    open()
                    closeAction = standardButton(Dialog.Close)
                    if (closeAction) {
                        closeAction.KeyNavigation.tab = files
                        closeAction.KeyNavigation.backtab = files
                        closeAction.forceActiveFocus()
                    }
                }
                onClosed: { root.detailRows = null; closeReview.forceActiveFocus() }
                Frame {
                    anchors.fill: parent
                    padding: 2
                    background: Rectangle {
                        color: Theme.surface
                        border.color: files.activeFocus ? Theme.accent : Theme.rule
                        border.width: files.activeFocus ? 2 : 1
                    }
                    ListView {
                        id: files
                        anchors.fill: parent
                        activeFocusOnTab: true
                        keyNavigationEnabled: false
                        Accessible.name: root.detailTitle
                        Accessible.description: qsTr("Use Up and Down to scroll by pixels, Page Up and Page Down to scroll by pages, and Tab to reach Close.")
                        KeyNavigation.tab: details.closeAction
                        KeyNavigation.backtab: details.closeAction
                        Keys.onPressed: event => FontPresentation.scrollKey(files, event, Math.max(24, Theme.body * 2))
                        clip: true
                        reuseItems: true
                        spacing: 12
                        model: root.detailRows || []
                        ScrollBar.vertical: ScrollBar {}
                        delegate: ColumnLayout {
                            id: fileRow
                            required property var modelData
                            width: files.width
                            readonly property bool original: modelData.file !== undefined
                            readonly property var file: original ? modelData.file : modelData
                            Label {
                                Layout.fillWidth: true
                                text: fileRow.file.sourceName
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                            }
                            Label {
                                Layout.fillWidth: true
                                text: fileRow.modelData.fontOrigin ? FontPresentation.source(fileRow.modelData.fontOrigin) : fileRow.original ? fileRow.modelData.originalPath : fileRow.modelData.sourcePath
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                                color: Theme.secondary
                            }
                            Label {
                                Layout.fillWidth: true
                                visible: fileRow.modelData.archive !== undefined
                                text: qsTr("Archive member: %1").arg((fileRow.modelData.archive || {}).decoderPath || "")
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                            }
                            Label {
                                Layout.fillWidth: true
                                text: qsTr("%1 bytes").arg(fileRow.file.size)
                                textFormat: Text.PlainText
                                color: Theme.secondary
                            }
                        }
                    }
                }
            }
        }
    }
}
