pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

Pane {
    id: root
    objectName: "libraryView"
    signal removalRequested(var ids, bool deleteLibrary)
    required property LibraryModel library
    required property string selectedId
    required property MediaViewState viewState
    property string presentationMode: "list"
    property int mediaEpoch: 0
    property bool viewerAllowed: true
    readonly property bool viewerOpen: results.viewerOpen
    readonly property bool popupOpen: categories.down || heroes.down
    readonly property string mediaDomain: viewState.canonical([library.dataDir, library.sourcePath, library.nativeMode, library.demoMode, library.viewedProfileId])
    signal presentationModeRequested(string mode)
    signal selectMod(string id)
    signal detailsRequested(string identity)
    signal importFilesRequested()
    signal browseRequested()
    signal applyRequested()
    signal importFolderRequested()
    signal upstreamImportRequested()
    signal installedRequested()
    signal artworkRequested()
    signal clearArtworkRequested()
    readonly property var healthReport: library.health.report || ({})
    readonly property var repairableMods: (healthReport.mods || []).filter(m => ["fixable", "partlyFixable", "originalFiles"].includes(m.status))
    property var fixQueue: []
    property string fixProfile: ""
    property string fixRoot: ""
    Timer {
        interval: 150; repeat: true; running: root.fixQueue.length > 0
        onTriggered: {
            if (root.library.viewedProfileId !== root.fixProfile || root.library.activeProfileId !== root.fixProfile || root.library.dataDir !== root.fixRoot) { root.fixQueue = []; return }
            if (root.library.busy || root.library.loading || !root.library.canPreviewApply) return
            const id = root.fixQueue[0]
            if (id === "review") { root.fixQueue = []; root.applyRequested(); return }
            if (root.library.setFixEnabled(id, true)) root.fixQueue = root.fixQueue.slice(1)
            else root.fixQueue = []
        }
    }
    property url pendingArtwork
    // No library of our own yet: welcome the user instead of showing errors.
    // Multi-select for bulk actions (IDs as keys of a plain object).
    property var markedIds: ({})
    readonly property int markedCount: Object.keys(markedIds).length
    readonly property int bulkLimit: 256
    function setMarked(id, on) {
        const next = Object.assign({}, markedIds)
        if (on) { if (Object.keys(next).length >= bulkLimit) return; next[id] = true }
        else delete next[id]
        markedIds = next
    }
    function markAllShown() {
        const next = {}
        for (let i = 0; i < library.count && i < bulkLimit; ++i) { const id = library.idAt(i); if (id) next[id] = true }
        markedIds = next
    }
    function clearMarks() { markedIds = ({}) }
    function bulkEnable(on) { if (library.setModsEnabled(Object.keys(markedIds), on)) clearMarks() }
    Connections {
        target: root.library
        function onSourceChanged() { root.clearMarks() }
    }
    readonly property bool firstRun: !library.demoMode && !library.nativeMode && !library.loading
                                     && library.bootstrapState === "absent"
    readonly property bool upstreamFound: !library.nativeMode && !library.demoMode && library.sourcePath !== ""
                                          && library.error === ""
    // Welcome replaces the screen only when nothing was found; real source errors still show.
    readonly property bool welcomeMode: firstRun && !upstreamFound && (library.error === "" || library.error.includes("sourceNotFound"))
    property var detailsFocusCard: null
    property string detailsFocusIdentity: ""
    property string detailsFocusContext: ""
    property int detailsFocusRevision: -1
    property int detailsFocusEpoch: -1
    function closeViewer() { results.closeViewer() }
    function setPresentation(mode) {
        if (mode === presentationMode) return
        results.beforeModeChange()
        presentationModeRequested(mode)
    }
    function snapshotFor(identity, revision, mediaRevision) {
        if (library.rowForId(identity) < 0) return null
        const mod = library.modDetails(identity)
        if (!mod || mod.id !== identity) return null
        const images = mod.images || []
        const sources = []
        for (let i = 0; i < images.length; ++i) sources.push(String(images[i]))
        return {identity: identity, source: sources.length ? sources[0] : "", sources: sources,
                hidden: !!mod.previewHidden, title: String(mod.title || "")}
    }
    function selectRow(identity, index) { results.selectIndex(index, identity) }
    function requestDetails(identity, index, card) {
        if (!visible || !enabled || results.viewerOpen || !viewerAllowed || !results.modelCoherent || !results.ready
                || !identity || index < 0 || library.rowForId(identity) !== index) return
        selectRow(identity, index)
        if (selectedId !== identity || library.selectedId !== identity || (library.selectedMod || {}).id !== identity) return
        detailsFocusCard = card
        detailsFocusIdentity = identity
        detailsFocusContext = results.contextKey
        detailsFocusRevision = results.changeRevision
        detailsFocusEpoch = mediaEpoch
        detailsRequested(identity)
    }
    function restoreDetailsFocus() {
        const card = detailsFocusCard
        const identity = detailsFocusIdentity
        const context = detailsFocusContext
        const revision = detailsFocusRevision
        const epoch = detailsFocusEpoch
        detailsFocusCard = null
        detailsFocusIdentity = ""
        detailsFocusContext = ""
        detailsFocusRevision = -1
        detailsFocusEpoch = -1
        if (!visible || !enabled || viewerOpen || !viewerAllowed || !results.ready || !results.modelCoherent) return
        if (card && context === results.contextKey && revision === results.changeRevision && epoch === mediaEpoch
                && identity === selectedId && library.selectedId === identity
                && library.rowForId(identity) >= 0 && card.anchorIdentity === identity && card.resultHost === results
                && card.interactable && card.visible && card.enabled && card.focusTitle()) return
        if (results.view && results.view.visible && results.view.enabled) results.view.forceActiveFocus()
    }
    padding: Theme.spacing24
    onVisibleChanged: library.setLibraryMediaActive(visible)
    Component.onCompleted: library.setLibraryMediaActive(visible)
    Component.onDestruction: library.setLibraryMediaActive(false)
    ButtonGroup { id: presentationButtons }
    Connections {
        target: root.library
        function onCategoriesChanged() {
            if (root.library.heroFilter !== "" && !root.library.heroes.includes(root.library.heroFilter))
                root.library.heroFilter = ""
            if (root.library.typeFilter !== "" && !root.library.types.includes(root.library.typeFilter))
                root.library.typeFilter = ""
        }
    }
    ColumnLayout {
        anchors.fill: parent
        spacing: Theme.spacing12
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing12
            RowLayout {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                spacing: Theme.spacing12
                Label { text: qsTr("Library"); font.family: Theme.displayFamily; font.pointSize: Theme.heading; font.weight: Font.DemiBold }
                Label {
                    objectName: "librarySummary"
                    Layout.fillWidth: true
                    visible: !root.welcomeMode
                    text: qsTr("%1 of %2 on")
                          .arg(root.library.enabledCount).arg(root.library.totalCount)
                          + (root.library.count !== root.library.totalCount ? qsTr(" · %1 shown").arg(root.library.count) : "")
                    textFormat: Text.PlainText
                    color: Theme.secondary
                    Layout.minimumWidth: 0
                    elide: Text.ElideRight
                }
                Label {
                    objectName: "sourcePathLabel"
                    Layout.fillWidth: true
                    visible: !root.library.nativeMode && root.library.sourcePath !== ""
                    text: qsTr("Reading %1").arg(root.library.sourcePath)
                    textFormat: Text.PlainText
                    color: Theme.faint
                    font.pointSize: Theme.small
                    elide: Text.ElideMiddle
                }
            }
            BusyIndicator {
                objectName: "loadingIndicator"
                Layout.preferredWidth: 24
                Layout.preferredHeight: 24
                running: root.library.loading
                visible: running
                Accessible.name: qsTr("Loading library")
            }
            StatusPill {
                visible: (root.library.loading || root.library.demoMode || root.library.readOnly) && !(root.welcomeMode)
                text: root.library.loading ? qsTr("Loading…") : root.library.demoMode ? qsTr("Demo · changes aren't saved") : qsTr("Read-only")
                tone: "muted"
            }
            LibrarySidebar {
                library: root.library
                onReviewRequested: root.applyRequested()
            }
            Button {
                objectName: "checkUpdatesButton"
                visible: root.library.nativeMode
                flat: true
                icon.source: Theme.icon("refresh")
                text: qsTr("Check for updates")
                enabled: !root.library.busy && !root.library.loading && !root.library.healthChecking && !root.library.updatesChecking && root.library.activeProfileId === root.library.viewedProfileId
                ToolTip.visible: hovered && root.library.activeProfileId !== root.library.viewedProfileId
                ToolTip.text: qsTr("Switch to the active profile to check updates and fixes together.")
                onClicked: { root.library.checkForUpdates(); root.library.checkModHealth() }
            }
            Button {
                objectName: "fixModsButton"
                flat: true
                visible: root.library.nativeMode && !root.library.healthChecking && root.repairableMods.length > 0
                text: qsTr("Fix mods")
                enabled: root.library.canPreviewApply && root.fixQueue.length === 0
                onClicked: { root.fixProfile = root.library.viewedProfileId; root.fixRoot = root.library.dataDir; root.fixQueue = root.repairableMods.map(m => m.modId) }
            }
            Button {
                id: addButton
                objectName: "addModsButton"
                visible: root.library.nativeMode && root.library.localImport.canPrepare
                text: qsTr("Add mods")
                icon.source: Theme.icon("plus")
                onClicked: addMenu.popup(addButton, 0, addButton.height + 4)
                Menu {
                    id: addMenu
                    MenuItem { text: qsTr("Files (VPK, ZIP, RAR, 7z)…"); icon.source: Theme.icon("upload"); onTriggered: root.importFilesRequested() }
                    MenuItem { text: qsTr("Folder…"); icon.source: Theme.icon("folder"); onTriggered: root.importFolderRequested() }
                    MenuItem { text: qsTr("Browse GameBanana"); icon.source: Theme.icon("compass"); onTriggered: root.browseRequested() }
                    MenuItem { objectName: "installedModsMenuItem"; visible: root.library.canScanInstalledMods; height: visible ? implicitHeight : 0; text: qsTr("Mods already in the game…"); icon.source: Theme.icon("search"); onTriggered: root.installedRequested() }
                    MenuSeparator {}
                    MenuItem { text: root.pendingArtwork.toString() ? qsTr("Change artwork for next import…") : qsTr("Artwork for next import…"); icon.source: Theme.icon("image"); onTriggered: root.artworkRequested() }
                    MenuItem { visible: root.pendingArtwork.toString() !== ""; text: qsTr("Clear artwork"); icon.source: Theme.icon("x"); onTriggered: root.clearArtworkRequested() }
                }
            }

        }
        BisectPanel { library: root.library; visible: !root.welcomeMode && active }
        Rectangle {
            objectName: "upstreamImportBanner"
            Layout.fillWidth: true
            visible: root.upstreamFound
            implicitHeight: upstreamRow.implicitHeight + Theme.spacing24
            radius: Theme.radius
            color: Theme.accentSoft
            RowLayout {
                id: upstreamRow
                anchors.fill: parent
                anchors.margins: Theme.spacing12
                spacing: Theme.spacing12
                ColorImage { source: Theme.icon("info"); sourceSize: Qt.size(18, 18); Layout.preferredWidth: 18; Layout.preferredHeight: 18; color: Theme.accentInk }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    Label { Layout.fillWidth: true; text: qsTr("These are your mods in Deadlock Mod Manager"); font.weight: Font.DemiBold; wrapMode: Text.Wrap }
                    Label { Layout.fillWidth: true; text: qsTr("You can look but not change them here. Import them to manage everything in this app; the originals stay untouched."); color: Theme.secondary; wrapMode: Text.Wrap }
                }
                Button {
                    highlighted: true
                    text: qsTr("Import…")
                    icon.source: Theme.icon("upload")
                    enabled: root.library.canImport
                    onClicked: root.upstreamImportRequested()
                }
            }
        }
        Item {
            objectName: "welcome"
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: root.welcomeMode
            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 48, 520)
                spacing: Theme.spacing16
                Rectangle {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.preferredWidth: 64
                    Layout.preferredHeight: 64
                    radius: 18
                    color: Theme.accent
                    Label { anchors.centerIn: parent; text: "D"; color: Theme.selectionText; font.pointSize: Theme.display; font.weight: Font.Black }
                }
                Label {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Welcome to Modlock")
                    font.pointSize: Theme.heading
                    font.weight: Font.DemiBold
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Create your library to start adding mods, even before installing Deadlock. With automatic Apply on, changes to your active mods are applied when the game is closed.")
                    color: Theme.secondary
                    wrapMode: Text.Wrap
                }
                Button {
                    objectName: "welcomeCreateLibraryButton"
                    Layout.alignment: Qt.AlignHCenter
                    highlighted: true
                    text: qsTr("Create library")
                    icon.source: Theme.icon("plus")
                    enabled: root.library.canCreateLibrary
                    onClicked: root.library.createLibrary()
                }
                Label {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("Already use Deadlock Mod Manager? Open its library file from the ⋯ menu to import your mods.")
                    color: Theme.faint
                    font.pointSize: Theme.small
                    wrapMode: Text.Wrap
                }
            }
        }
        Rectangle {
            objectName: "selectionBar"
            Layout.fillWidth: true
            visible: root.markedCount > 0
            implicitHeight: selectionActions.implicitHeight + Theme.spacing16
            radius: Theme.radius
            color: Theme.accentSoft
            border.color: Qt.alpha(Theme.accent, 0.5)
            Flow {
                id: selectionActions
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: Theme.spacing8
                spacing: Theme.spacing8
                Label {
                    height: bulkEnableAction.implicitHeight
                    verticalAlignment: Text.AlignVCenter
                    text: qsTr("%n selected", "", root.markedCount)
                    font.weight: Font.DemiBold
                }
                Button { flat: true; text: qsTr("Select all shown"); visible: root.markedCount < Math.min(root.library.count, root.bulkLimit); onClicked: root.markAllShown() }
                Button { id: bulkEnableAction; objectName: "bulkEnableButton"; text: qsTr("Turn on"); icon.source: Theme.icon("check"); enabled: root.library.canBulkToggle; onClicked: root.bulkEnable(true) }
                Button { objectName: "bulkDisableButton"; text: qsTr("Turn off"); enabled: root.library.canBulkToggle; onClicked: root.bulkEnable(false) }
                Button { objectName: "bulkRemoveButton"; text: qsTr("Remove from profile…"); icon.source: Theme.icon("trash"); icon.color: Theme.danger; enabled: root.library.canBulkOperate; onClicked: root.removalRequested(Object.keys(root.markedIds), false) }
                Button { objectName: "bulkDeleteButton"; text: qsTr("Delete…"); icon.source: Theme.icon("trash"); icon.color: Theme.danger; enabled: root.library.canDeleteMods; onClicked: root.removalRequested(Object.keys(root.markedIds), true) }
                ToolButton { icon.source: Theme.icon("x"); display: AbstractButton.IconOnly; text: qsTr("Clear selection"); onClicked: root.clearMarks() }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: !(root.welcomeMode)
            spacing: Theme.spacing8
            TextField {
                id: search
                objectName: "modSearch"
                Layout.fillWidth: true
                Layout.minimumWidth: 200
                leftPadding: 36
                placeholderText: qsTr("Search mods…")
                Accessible.name: qsTr("Search mods, authors or heroes")
                KeyNavigation.tab: categories
                text: root.library.query
                onActiveFocusChanged: if (activeFocus && !results.restoring) {
                    root.library.viewFocus = "search"
                    results.queueCapture()
                }
                onTextEdited: root.library.query = text
                Keys.onEscapePressed: { clear(); root.library.query = "" }
                ColorImage {
                    x: 12
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    source: Theme.icon("search")
                    sourceSize: Qt.size(16, 16)
                    color: Theme.faint
                }
            }
            Rectangle {
                Layout.preferredHeight: 34
                Layout.preferredWidth: viewToggle.implicitWidth + 4
                radius: Theme.radius4
                color: Theme.surface
                border.color: Theme.rule
                Row {
                    id: viewToggle
                    anchors.centerIn: parent
                    spacing: 2
                    ToolButton {
                        id: listViewButton
                        objectName: "listViewButton"
                        implicitWidth: 30
                        implicitHeight: 28
                        checkable: true
                        checked: root.presentationMode === "list"
                        ButtonGroup.group: presentationButtons
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon("list")
                        icon.width: 16
                        icon.height: 16
                        text: qsTr("List")
                        Accessible.name: qsTr("List view")
                        ToolTip { visible: parent.hovered; text: qsTr("List view") }
                        onClicked: root.setPresentation("list")
                    }
                    ToolButton {
                        objectName: "gridViewButton"
                        KeyNavigation.tab: results.view
                        implicitWidth: 30
                        implicitHeight: 28
                        checkable: true
                        checked: root.presentationMode === "grid"
                        ButtonGroup.group: presentationButtons
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon("grid")
                        icon.width: 16
                        icon.height: 16
                        text: qsTr("Grid")
                        Accessible.name: qsTr("Grid view")
                        ToolTip { visible: parent.hovered; text: qsTr("Grid view") }
                        onClicked: root.setPresentation("grid")
                    }
                }
            }
        }
        Flow {
            objectName: "libraryFilterFlow"
            Layout.fillWidth: true
            visible: !root.welcomeMode
            spacing: Theme.spacing8
            ComboBox {
                id: categories
                objectName: "categoryChooser"
                width: Math.min(Math.max(130, categoryMetrics.advanceWidth + leftPadding + rightPadding), root.width * 0.3)
                model: [{ label: qsTr("All types"), category: "" }].concat(root.library.types.map(category => ({ label: category, category: category })))
                textRole: "label"
                valueRole: "category"
                currentIndex: root.library.typeFilter === "" ? 0 : root.library.types.indexOf(root.library.typeFilter) + 1
                Accessible.name: qsTr("Mod type")
                onActivated: root.library.typeFilter = currentValue
                contentItem: Label {
                    objectName: "categoryText"
                    text: categories.displayText
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    verticalAlignment: Text.AlignVCenter
                }
                delegate: ItemDelegate {
                    id: categoryOption
                    required property var modelData
                    required property int index
                    width: categories.width
                    highlighted: categories.highlightedIndex === index
                    contentItem: Label { text: categoryOption.modelData.label; textFormat: Text.PlainText; elide: Text.ElideRight }
                }
            }
            TextMetrics { id: categoryMetrics; font: categories.font; text: categories.displayText }
            ComboBox {
                id: heroes
                objectName: "heroChooser"
                width: Math.min(Math.max(130, heroMetrics.advanceWidth + leftPadding + rightPadding), root.width * 0.3)
                model: [qsTr("All heroes")].concat(root.library.heroes)
                currentIndex: root.library.heroFilter === "" ? 0 : root.library.heroes.indexOf(root.library.heroFilter) + 1
                Accessible.name: qsTr("Mod hero")
                onActivated: root.library.heroFilter = currentIndex ? root.library.heroes[currentIndex - 1] : ""
                contentItem: Label { text: heroes.displayText; textFormat: Text.PlainText; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
                delegate: ItemDelegate {
                    required property string modelData
                    required property int index
                    width: heroes.width
                    highlighted: heroes.highlightedIndex === index
                    contentItem: Label { text: modelData; textFormat: Text.PlainText; elide: Text.ElideRight }
                }
            }
            TextMetrics { id: heroMetrics; font: heroes.font; text: heroes.displayText }
            Rectangle { width: 1; height: heroes.height; color: Theme.ruleStrong }
            FilterChip {
                objectName: "enabledFilter"
                checkable: true
                text: qsTr("On only")
                count: root.library.enabledCount
                icon.source: checked ? Theme.icon("check") : ""
                checked: root.library.enabledOnly
                onToggled: root.library.enabledOnly = checked
            }
            FilterChip {
                objectName: "needsAttentionFilter"
                checkable: true
                text: qsTr("Needs attention")
                checked: root.library.needsAttentionOnly
                onToggled: root.library.needsAttentionOnly = checked
            }
            FilterChip {
                objectName: "updatesFilter"
                KeyNavigation.tab: updateAllButton.visible && updateAllButton.enabled ? updateAllButton : listViewButton
                checkable: true
                text: qsTr("Updates")
                checked: root.library.updatesOnly
                onToggled: root.library.updatesOnly = checked
            }
            Button {
                id: updateAllButton
                objectName: "updateAllButton"
                KeyNavigation.tab: listViewButton
                visible: root.library.updateCount > 0
                text: qsTr("Update all (%1)").arg(root.library.updateCount)
                enabled: !root.library.busy && root.library.updatingCount === 0
                onClicked: root.library.updateAll()
            }
        }
        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: !root.welcomeMode
            color: root.presentationMode === "list" ? Theme.surface : "transparent"
            radius: Theme.radiusCard
            border.width: root.presentationMode === "list" ? 1 : 0
            border.color: Theme.rule
            clip: true
            MediaResultView {
                id: results
                anchors.fill: parent
                anchors.margins: root.presentationMode === "list" ? 1 : 0
                objectName: "modList"
                visible: !(root.welcomeMode)
                stateBank: root.viewState
                model: root.library
                scope: "library"
                domain: root.mediaDomain
                contextKey: root.viewState.canonical([root.mediaDomain, root.library.query, root.library.category, root.library.heroFilter, root.library.typeFilter, root.library.enabledOnly])
                selectedIdentity: root.selectedId
                focusKey: root.library.viewFocus
                presentationMode: root.presentationMode
                ownerEpoch: root.mediaEpoch
                initialScroll: root.library.viewScrollPosition
                ready: !root.library.loading
                viewerAllowed: root.viewerAllowed
                indexForIdentity: identity => root.library.rowForId(identity)
                identityAt: index => root.library.idAt(index)
                snapshotFor: (identity, revision) => root.snapshotFor(identity, revision)
                delegateComponent: Component {
                    ModRow {
                        id: entry
                        required property var mediaRevision
                        resultHost: results
                        ownerSnapshot: root.snapshotFor(modId, results.modelRevision, mediaRevision)
                        mediaDemandOwner: root.library
                        selected: modId === root.selectedId
                        readOnly: root.library.readOnly || (root.library.nativeMode && !root.library.canToggleEnabled)
                        nativeMode: root.library.nativeMode
                        canSetEnabled: root.library.canToggleEnabled
                        profileName: (root.library.profiles.find(profile => profile.id === root.library.viewedProfileId) || {}).name || qsTr("Default")
                        onDetailsRequested: root.requestDetails(modId, index, entry)
                        onToggleRequested: enabled => root.library.setModEnabled(modId, enabled)
                        selectable: root.library.canBulkToggle
                        marked: !!root.markedIds[modId]
                        markMode: root.markedCount > 0
                        onMarkToggled: checked => root.setMarked(modId, checked)
                        onMediaDemandRequested: (consumer, identity, wanted) => root.library.demandModMedia(consumer, identity, wanted)
                    }
                }
                onSelectionRequested: identity => root.selectMod(identity)
                onDetailsRequested: identity => root.requestDetails(identity, root.library.rowForId(identity), null)
                onPositionChanged: (position, focus) => {
                    if (root.library.viewScrollPosition !== position) root.library.viewScrollPosition = position
                    if (root.library.viewFocus !== focus) root.library.viewFocus = focus
                }
                onSearchFocusRequested: search.forceActiveFocus()
                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.min(parent.width, 420)
                    visible: root.library.count === 0 && !root.library.loading && root.library.error === ""
                             && !root.library.outcomeUnknown && !root.library.recoveryRequired
                             && (root.library.demoMode || !!root.library.viewedProfileId)
                    EmptyState {
                        objectName: "emptyLabel"
                        Layout.fillWidth: true
                        headline: root.library.totalCount === 0 ? (root.library.demoMode ? qsTr("No sample mods.") : qsTr("This profile has no mods.")) : qsTr("No mods match these filters.")
                    }
                    Button {
                        Layout.alignment: Qt.AlignHCenter
                        visible: root.library.totalCount > 0
                        text: qsTr("Clear filters")
                        onClicked: { search.clear(); root.library.query = ""; root.library.category = ""; root.library.heroFilter = ""; root.library.typeFilter = ""; root.library.enabledOnly = false; root.library.needsAttentionOnly = false
                            root.library.updatesOnly = false; search.forceActiveFocus() }
                    }
                    RowLayout {
                        Layout.alignment: Qt.AlignHCenter
                        visible: root.library.totalCount === 0 && !root.library.demoMode
                        Button {
                            text: qsTr("Import files…")
                            visible: root.library.nativeMode && root.library.localImport.canPrepare
                            highlighted: true
                            onClicked: root.importFilesRequested()
                        }
                        Button { text: qsTr("Browse catalog"); onClicked: root.browseRequested() }
                        Button {
                            objectName: "emptyInstalledMods"
                            visible: root.library.canScanInstalledMods && (root.library.gamePath || root.library.detectedGamePath) !== ""
                            text: qsTr("Find mods already in the game")
                            icon.source: Theme.icon("search")
                            onClicked: root.installedRequested()
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.library.totalCount === 0 && root.library.nativeMode && root.library.localImport.canPrepare
                        text: qsTr("Or drop VPK files, ZIP, RAR or 7z archives, or a folder.")
                        textFormat: Text.PlainText
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        color: Theme.secondary
                        font.pointSize: Theme.small
                    }
                }
            }
        }
    }
    Shortcut { sequences: [StandardKey.Find]; onActivated: search.forceActiveFocus() }
    Shortcut { sequence: "Escape"; enabled: root.markedCount > 0; onActivated: root.clearMarks() }

}
