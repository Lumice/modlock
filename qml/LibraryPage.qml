import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import Deadlock.Native

Item {
    id: root
    required property LibraryModel library
    required property MediaViewState viewState
    property bool fontFocusReturnAllowed: false
    property string presentationMode: "list"
    signal presentationModeRequested(string mode)
    signal remoteRequested(var submission)
    signal changelogRequested(var submission)
    signal authorRequested(string authorId)
    signal replacementRequested(string modId, string action)
    signal importFilesRequested()
    signal browseRequested()
    signal applyRequested()
    signal importFolderRequested()
    signal upstreamImportRequested()
    signal installedRequested()
    signal artworkRequested()
    signal clearArtworkRequested()
    property url pendingArtwork
    property bool detailsOpen: false
    property bool detailsFromRow: false
    property int detailsOpenRevision: 0
    property int mediaEpoch: 0
    readonly property string mediaContext: JSON.stringify([library.viewedProfileId, library.selectedId, mediaEpoch])
    readonly property bool hasSelection: !!library.selectedId && !!inspector.mod && inspector.mod.id === library.selectedId
    readonly property bool viewerOpen: libraryView.viewerOpen || inspector.viewerOpen
                                       || (detailsLoader.status === Loader.Ready && detailsLoader.item.viewerOpen)
    readonly property bool popupOpen: detailsOpen || viewerOpen || libraryView.popupOpen || removalDialog.visible
    function bulkOffForScreenshot() { markForScreenshot(); libraryView.bulkEnable(false) }
    function markForScreenshot() { libraryView.setMarked(library.idAt(0), true); libraryView.setMarked(library.idAt(1), true) }
    function closeViewer() {
        if (libraryView.viewerOpen) libraryView.closeViewer()
        else if (inspector.viewerOpen) inspector.closeViewer()
        else if (detailsLoader.status === Loader.Ready && detailsLoader.item.viewerOpen) detailsLoader.item.closeViewer()
    }
    function focusDownloads() {
        if (detailsLoader.status === Loader.Ready) detailsLoader.item.focusDownloads()
        else if (inspector.visible) inspector.focusDownloads()
    }
    function closeDetails() {
        if (!detailsOpen) return
        const fromRow = detailsFromRow
        const revision = detailsOpenRevision
        detailsOpen = false
        detailsFromRow = false
        Qt.callLater(() => {
            if (!root || !root.visible || !root.enabled || !root.fontFocusReturnAllowed
                    || root.popupOpen || root.detailsOpenRevision !== revision) return
            if (fromRow) libraryView.restoreDetailsFocus()
            else if (detailsButton.visible && detailsButton.enabled) detailsButton.forceActiveFocus()
            else libraryView.restoreDetailsFocus()
        })
    }
    onDetailsOpenChanged: if (detailsOpen) ++detailsOpenRevision
    onWidthChanged: if (detailsOpen && width >= 926) closeDetails()
    onHasSelectionChanged: if (!hasSelection && detailsOpen) closeDetails()
    Connections {
        target: root.library
        function onSourceChanged() { ++root.mediaEpoch }
    }
    RowLayout {
        anchors.fill: parent
        spacing: 0
        LibraryView {
            id: libraryView
            Layout.fillWidth: true
            Layout.fillHeight: true
            library: root.library
            selectedId: root.library.selectedId
            viewState: root.viewState
            presentationMode: root.presentationMode
            mediaEpoch: root.mediaEpoch
            viewerAllowed: !inspector.viewerOpen && !(detailsLoader.status === Loader.Ready && detailsLoader.item.viewerOpen)
            onRemovalRequested: (ids, deleteLibrary) => removalDialog.openFor(ids, deleteLibrary)
            onPresentationModeRequested: mode => root.presentationModeRequested(mode)
            onSelectMod: id => root.library.selectedId = id
            onImportFilesRequested: root.importFilesRequested()
            onBrowseRequested: root.browseRequested()
            onApplyRequested: root.applyRequested()
            onImportFolderRequested: root.importFolderRequested()
            onUpstreamImportRequested: root.upstreamImportRequested()
            onInstalledRequested: root.installedRequested()
            onArtworkRequested: root.artworkRequested()
            onClearArtworkRequested: root.clearArtworkRequested()
            pendingArtwork: root.pendingArtwork
            onDetailsRequested: identity => {
                if (root.width < 926 && !root.viewerOpen && root.library.selectedId === identity) {
                    root.detailsFromRow = true
                    root.detailsOpen = true
                }
            }
        }
        Button {
            id: detailsButton
            // Narrow layouts open details from the card title or a double-click.
            visible: false
            text: qsTr("Details…")
            enabled: root.hasSelection && !root.viewerOpen
            onClicked: { root.detailsFromRow = false; root.detailsOpen = true }
        }
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 1
            visible: inspector.visible
            color: Theme.rule
        }
        ModDetails {
            id: inspector
            Layout.preferredWidth: 330
            Layout.fillHeight: true
            visible: root.width >= 926 && root.hasSelection
            enabled: !libraryView.viewerOpen
            library: root.library
            mod: root.library.selectedMod
            mediaContextKey: root.mediaContext
            fontFocusReturnAllowed: root.fontFocusReturnAllowed && !root.viewerOpen && !root.detailsOpen && !libraryView.popupOpen
            readOnly: root.library.readOnly || root.library.busy || root.library.outcomeUnknown || root.library.recoveryRequired || (root.library.nativeMode && !root.library.canSetEnabled)
            toggleReadOnly: root.library.readOnly || (root.library.nativeMode && !root.library.canToggleEnabled)
            nativeMode: root.library.nativeMode
            fonts: root.library.fonts
            options: root.library.options
            remote: root.library.remote
            onRemoteRequested: submission => root.remoteRequested(submission)
            onChangelogRequested: submission => root.changelogRequested(submission)
            onAuthorRequested: authorId => root.authorRequested(authorId)
            onReplacementRequested: (modId, action) => root.replacementRequested(modId, action)
            onRemovalRequested: (modId, deleteLibrary) => removalDialog.openFor([modId], deleteLibrary)
            onToggleRequested: (id, enabled) => root.library.setModEnabled(id, enabled)
        }
    }
    Loader {
        id: detailsLoader
        active: root.detailsOpen && root.width < 926 && root.hasSelection
        sourceComponent: Dialog {
            readonly property bool viewerOpen: detailsInspector.viewerOpen
            function closeViewer() { detailsInspector.closeViewer() }
            function focusDownloads() { detailsInspector.focusDownloads() }
            parent: Overlay.overlay
            anchors.centerIn: parent
            width: Math.min(540, parent ? parent.width - Theme.spacing32 : 540)
            height: Math.min(680, parent ? parent.height - Theme.spacing32 : 680)
            modal: false
            title: qsTr("Mod details")
            standardButtons: Dialog.Close
            Component.onCompleted: open()
            onClosed: root.closeDetails()
            ModDetails {
                id: detailsInspector
                anchors.fill: parent
                enabled: !libraryView.viewerOpen
                library: root.library
                mod: root.library.selectedMod
                mediaContextKey: root.mediaContext
                fontFocusReturnAllowed: root.fontFocusReturnAllowed && !root.viewerOpen && root.detailsOpen && root.width < 926 && !libraryView.popupOpen
                readOnly: root.library.readOnly || root.library.busy || root.library.outcomeUnknown || root.library.recoveryRequired || (root.library.nativeMode && !root.library.canSetEnabled)
                toggleReadOnly: root.library.readOnly || (root.library.nativeMode && !root.library.canToggleEnabled)
                nativeMode: root.library.nativeMode
                fonts: root.library.fonts
                options: root.library.options
                remote: root.library.remote
                onRemoteRequested: submission => root.remoteRequested(submission)
            onChangelogRequested: submission => root.changelogRequested(submission)
            onAuthorRequested: authorId => root.authorRequested(authorId)
                onReplacementRequested: (modId, action) => root.replacementRequested(modId, action)
                onRemovalRequested: (modId, deleteLibrary) => removalDialog.openFor([modId], deleteLibrary)
                onToggleRequested: (id, enabled) => root.library.setModEnabled(id, enabled)
            }
        }
    }
    ModRemovalDialog { id: removalDialog; library: root.library }

}

