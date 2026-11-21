pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle
import "HomeState.js" as HomeState

FocusScope {
    id: root
    required property LibraryModel library
    property MediaViewState viewState: null
    property bool globalExternalNoticeVisible: false
    property CatalogController catalog: library ? library.catalog : null
    signal applyRequested()
    signal libraryRequested()
    signal installedRequested()
    signal browseRequested()
    signal browseSortRequested(string sort)
    signal submissionRequested(var submission)

    readonly property string gameState: HomeState.state(library)
    readonly property bool idle: !library.loading && !library.busy && !library.writeBusy && !library.launch.writeGate
    readonly property var repairable: ((library.health.report || {}).mods || []).filter(m => ["fixable", "partlyFixable", "originalFiles"].includes(m.status))
    readonly property int updateCount: typeof library.updateCount !== "undefined" ? (library.updateCount ?? 0) : 0
    readonly property int updatingCount: typeof library.updatingCount !== "undefined" ? (library.updatingCount ?? 0) : 0
    readonly property var unimported: library.installedMods.filter(m => !m.alreadyInLibrary)
    readonly property var recentMods: (library.recentMods || []).slice(0, 5)
    readonly property var latestRows: catalog ? (catalog.homeLatest || []).slice(0, 5) : []
    readonly property var popularRows: catalog ? (catalog.homePopular || []).slice(0, 5) : []
    readonly property var homeProvenance: catalog ? (catalog.homeProvenance || ({})) : ({})
    readonly property bool homeLoading: catalog ? !!catalog.homeLoading : false
    readonly property bool catalogUnavailable: catalog ? (!!catalog.homeUnavailable || (!catalog.available && library.nativeMode)) : false
    readonly property bool catalogIncomplete: !!homeProvenance.incomplete
    readonly property bool catalogOutdated: !!homeProvenance.stale || !!homeProvenance.refreshError
    readonly property bool showStatusNotice: ["viewed", "choices", "viewedChoices"].includes(gameState) || (gameState === "unknown" && !library.error) || (gameState === "external" && !globalExternalNoticeVisible)
    readonly property string homeScopeHash: Qt.md5([library.dataDir || "", library.sourcePath || "", library.nativeMode, library.demoMode].join("|"))
    readonly property int shelfColumns: contentColumn.width < 820 ? 3 : 5
    readonly property real cardWidth: Math.max(140, Math.floor((contentColumn.width - Theme.spacing12 * (shelfColumns - 1)) / shelfColumns))

    property bool homeEnterConsumed: false
    property bool homeEnterArmed: false
    property int homeEnterGeneration: 0
    function stopHomeEnter() {
        homeEnterTimer.stop()
        homeEnterArmed = false
    }
    function evaluateHomeEnter() {
        const hasTiles = recentMods.length > 0 || latestRows.length > 0 || popularRows.length > 0
        if (homeEnterConsumed || !visible || !(library.nativeMode || library.demoMode) || library.loading) return
        if (!hasTiles && homeLoading) return
        homeEnterConsumed = true
        const alreadyVisited = !!viewState && viewState.hasEnteredPage("home")
        if (viewState) viewState.markPageEntered("home")
        if (alreadyVisited || !hasTiles || Tokens.reduceMotion || Tokens.normal <= 0) return
        ++homeEnterGeneration
        homeEnterArmed = true
        homeEnterTimer.restart()
    }
    Timer {
        id: homeEnterTimer
        interval: 600
        onTriggered: root.homeEnterArmed = false
    }
    Connections {
        target: Tokens
        function onReduceMotionChanged() { if (Tokens.reduceMotion) root.stopHomeEnter() }
    }

    property var mediaDemands: ({})
    function demandArtwork() {
        library.setLibraryMediaActive(visible)
        const nextIds = (visible && (library.nativeMode || library.demoMode))
            ? Array.from(new Set(recentMods.map(m => String(m.id || "")).filter(Boolean)))
            : []
        const prev = mediaDemands
        const next = ({})
        for (const id of Object.keys(prev)) {
            if (!nextIds.includes(id)) library.demandModMedia(prev[id], id, false)
        }
        for (const id of nextIds) {
            const consumer = prev[id] || ("home:" + Qt.md5(id))
            next[id] = consumer
            library.demandModMedia(consumer, id, true)
        }
        mediaDemands = next
    }
    onVisibleChanged: { Qt.callLater(demandArtwork); if (!visible) stopHomeEnter(); else evaluateHomeEnter() }
    onRecentModsChanged: { Qt.callLater(demandArtwork); evaluateHomeEnter() }
    onLatestRowsChanged: evaluateHomeEnter()
    onPopularRowsChanged: evaluateHomeEnter()
    onHomeLoadingChanged: evaluateHomeEnter()
    Connections { target: root.library; function onSourceChanged() { Qt.callLater(root.demandArtwork) } function onLoadingChanged() { root.evaluateHomeEnter() } }
    Component.onCompleted: { evaluateHomeEnter(); Qt.callLater(demandArtwork) }
    Component.onDestruction: {
        stopHomeEnter()
        if (viewState && (library.nativeMode || library.demoMode) && !library.loading) viewState.markPageEntered("home")
        for (const id of Object.keys(mediaDemands)) library.demandModMedia(mediaDemands[id], id, false)
        mediaDemands = ({})
        library.setLibraryMediaActive(false)
    }

    property var fixQueue: []
    property string fixProfile: ""
    property string fixRoot: ""
    function activateStatusAction() {
        if (["external", "viewed"].includes(gameState)) applyRequested()
        else libraryRequested()
    }
    function catalogContextKey(shelf, identity) {
        const epoch = catalog ? catalog.homeEpoch : 0
        return "home-c-" + homeScopeHash + ":" + epoch + ":" + shelf + ":" + Qt.md5(String(identity || ""))
    }
    // Read the actual layout positions so a position-only change is reactive.
    // Home uses untransformed, axis-aligned layouts inside this scroll content.
    function tileInViewport(tile, viewportY, viewportHeight) {
        if (viewportHeight <= 0) return false
        let top = 0
        let ancestor = tile
        while (ancestor) {
            top += ancestor.y
            if (ancestor === contentColumn) break
            ancestor = ancestor.parent
        }
        if (!ancestor) return false
        return top + tile.height > viewportY - 80 && top < viewportY + viewportHeight + 80
    }
    function compactDownloads(value) {
        const number = Number(value)
        if (!isFinite(number) || number <= 0) return ""
        if (number >= 1000000) return qsTr("%1m").arg((number / 1000000).toFixed(1).replace(/\.0$/, ""))
        if (number >= 1000) return qsTr("%1k").arg((number / 1000).toFixed(1).replace(/\.0$/, ""))
        return catalog ? catalog.displayCount(String(value)) : String(Math.round(number))
    }
    function latestSubtitle(row) {
        const parts = [Theme.meaningful(row.hero), Theme.meaningful(row.category), String(row.author || "").trim()].filter(Boolean)
        return parts.slice(0, 2).join(" · ")
    }
    function popularSubtitle(row) {
        const dl = compactDownloads(row.downloads)
        const parts = [
            dl ? qsTr("%1 downloads").arg(dl) : "",
            Theme.meaningful(row.hero) || Theme.meaningful(row.category) || String(row.author || "").trim()
        ].filter(Boolean)
        return parts.join(" · ")
    }
    function catalogQualifierText() {
        const parts = [qsTr("From your saved catalog")]
        if (catalogOutdated) parts.push(qsTr("Saved results may be out of date."))
        return parts.join(" · ")
    }

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

    ColumnLayout {
        objectName: "homeWelcome"
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 560)
        visible: !root.library.nativeMode && !root.library.demoMode
        spacing: Theme.spacing16
        Label { text: qsTr("Home"); font.pointSize: Theme.heading; font.weight: Font.DemiBold }
        Label { Layout.fillWidth: true; text: qsTr("Welcome to Modlock"); font.pointSize: Theme.heading; wrapMode: Text.Wrap }
        Label { Layout.fillWidth: true; text: qsTr("Keep your mods together in a library. You can start collecting before you install Deadlock. With automatic Apply on, changes to your active mods are applied when the game is closed."); wrapMode: Text.Wrap; color: Theme.secondary }
        Button { objectName: "homeCreateLibraryButton"; text: qsTr("Create library"); highlighted: true; enabled: root.library.canCreateLibrary; onClicked: root.library.createLibrary() }
        Button { text: qsTr("Open library"); onClicked: root.libraryRequested() }
        Label { Layout.fillWidth: true; text: qsTr("Already use Deadlock Mod Manager? Import its library from the ⋯ menu."); wrapMode: Text.Wrap; color: Theme.secondary }
    }

    ScrollView {
        id: homeScroll
        objectName: "homePage"
        readonly property int shelfColumns: root.shelfColumns
        visible: root.library.nativeMode || root.library.demoMode
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            id: contentColumn
            objectName: "homeShelfContent"
            width: Math.max(240, homeScroll.availableWidth - Theme.spacing24 * 2)
            x: Theme.spacing24
            spacing: Theme.spacing16
            Item { Layout.preferredHeight: Theme.spacing8 }
            SectionHeading {
                text: qsTr("Home")
                pointSize: Theme.heading
            }

            Callout {
                objectName: "homeStatusCallout"
                Layout.fillWidth: true
                visible: root.showStatusNotice
                tone: ["external", "recovery", "unknown"].includes(root.gameState) ? "warning" : "info"
                title: HomeState.sentence(root.gameState)
                Button {
                    objectName: "homeStatusAction"
                    text: HomeState.action(root.gameState)
                    enabled: root.idle && (["external", "viewed"].includes(root.gameState) ? root.library.canPreviewApply : true)
                    onClicked: root.activateStatusAction()
                }
            }

            Callout {
                objectName: "homeUnimportedNotice"
                Layout.fillWidth: true
                visible: root.unimported.length > 0
                tone: "warning"
                title: root.unimported.length === 1 ? qsTr("1 mod in the game hasn't been imported") : qsTr("%1 mods in the game haven't been imported").arg(root.unimported.length)
                Button { text: qsTr("Review…"); enabled: root.idle; onClicked: root.installedRequested() }
            }

            Callout {
                objectName: "homeUpdates"
                Layout.fillWidth: true
                visible: root.updateCount > 0 || root.updatingCount > 0
                tone: "info"
                title: root.updatingCount > 0
                    ? (root.updatingCount === 1 ? qsTr("Updating 1 mod…") : qsTr("Updating %1 mods…").arg(root.updatingCount))
                    : (root.updateCount === 1 ? qsTr("1 update available") : qsTr("%1 updates available").arg(root.updateCount))
                Button {
                    text: qsTr("View")
                    onClicked: {
                        if (typeof root.library.updatesOnly !== "undefined") {
                            root.library.query = ""
                            root.library.category = ""
                            root.library.heroFilter = ""
                            root.library.typeFilter = ""
                            root.library.enabledOnly = false
                            root.library.needsAttentionOnly = false
                            root.library.updatesOnly = true
                        }
                        root.libraryRequested()
                    }
                }
            }

            Callout {
                objectName: "homeFixes"
                Layout.fillWidth: true
                visible: root.repairable.length > 0 && root.library.activeProfileId === root.library.viewedProfileId
                tone: "warning"
                title: root.repairable.length === 1 ? qsTr("1 mod has fixes for this patch") : qsTr("%1 mods have fixes for this patch").arg(root.repairable.length)
                Button {
                    text: qsTr("Fix mods")
                    enabled: root.idle && root.library.canPreviewApply && root.fixQueue.length === 0
                    onClicked: {
                        root.fixProfile = root.library.viewedProfileId
                        root.fixRoot = root.library.dataDir
                        root.fixQueue = root.repairable.map(m => m.modId)
                    }
                }
            }

            ColumnLayout {
                objectName: "homeRecentSection"
                visible: root.recentMods.length > 0
                Layout.fillWidth: true
                spacing: Theme.spacing12
                SectionHeading {
                    text: qsTr("Recently added")
                }
                GridLayout {
                    objectName: "homeRecentGrid"
                    Layout.fillWidth: true
                    columns: root.shelfColumns
                    columnSpacing: Theme.spacing12
                    rowSpacing: Theme.spacing12
                    Repeater {
                        model: root.recentMods
                        delegate: HomeModTile {
                            required property var modelData
                            required property int index
                            id: recentTile
                            objectName: "homeRecentTile"
                            poster: true
                            enterIndex: index
                            enterArmed: root.homeEnterArmed
                            enterGeneration: root.homeEnterGeneration
                            Layout.preferredWidth: root.cardWidth
                            Layout.maximumWidth: root.cardWidth
                            Layout.fillHeight: true
                            title: modelData.title || ""
                            showSubtitle: false
                            imageSource: (modelData.images || [])[0] || ""
                            masked: !!modelData.previewHidden
                            previewActive: root.visible && root.tileInViewport(recentTile, homeScroll.contentItem.contentY, homeScroll.availableHeight)
                            contextKey: "home-r-" + root.homeScopeHash + ":" + Qt.md5(String(modelData.id || ""))
                            onClicked: {
                                root.library.query = ""
                                root.library.category = ""
                                root.library.heroFilter = ""
                                root.library.typeFilter = ""
                                root.library.enabledOnly = false
                                root.library.needsAttentionOnly = false
                                root.library.updatesOnly = false
                                root.library.selectedId = modelData.id
                                root.libraryRequested()
                            }
                        }
                    }
                }
            }

            ColumnLayout {
                objectName: "homeCatalogSection"
                Layout.fillWidth: true
                spacing: Theme.spacing16

                Label {
                    objectName: "homeCatalogQualifier"
                    visible: root.latestRows.length > 0 || root.popularRows.length > 0
                    Layout.fillWidth: true
                    text: root.catalogQualifierText()
                    textFormat: Text.PlainText
                    font.pointSize: Theme.small
                    color: Theme.secondary
                    wrapMode: Text.Wrap
                }

                Label {
                    objectName: "homeCatalogLoading"
                    visible: root.homeLoading && root.latestRows.length === 0 && root.popularRows.length === 0
                    Layout.fillWidth: true
                    text: qsTr("Reading saved catalog…")
                    textFormat: Text.PlainText
                    font.pointSize: Theme.small
                    color: Theme.secondary
                }

                ColumnLayout {
                    objectName: "homeLatestSection"
                    visible: root.latestRows.length > 0
                    Layout.fillWidth: true
                    spacing: Theme.spacing12
                    SectionHeading {
                        text: qsTr("Latest mods")
                        Button {
                            objectName: "homeLatestSeeAll"
                            text: qsTr("See all")
                            flat: true
                            Accessible.name: qsTr("See all latest mods")
                            onClicked: root.browseSortRequested("releaseDate")
                        }
                    }
                    GridLayout {
                        objectName: "homeLatestGrid"
                        Layout.fillWidth: true
                        columns: root.shelfColumns
                        columnSpacing: Theme.spacing12
                        rowSpacing: Theme.spacing12
                        Repeater {
                            model: root.latestRows
                            delegate: HomeModTile {
                                required property var modelData
                                required property int index
                                id: latestTile
                                objectName: "homeLatestTile"
                                enterIndex: root.recentMods.length + index
                                enterArmed: root.homeEnterArmed
                                enterGeneration: root.homeEnterGeneration
                                Layout.preferredWidth: root.cardWidth
                                Layout.maximumWidth: root.cardWidth
                                Layout.fillHeight: true
                                title: modelData.title || ""
                                subtitle: root.latestSubtitle(modelData)
                                showSubtitle: true
                                imageSource: (modelData.images || [])[0] || modelData.thumbnail || ""
                                masked: modelData.masked !== false
                                previewActive: root.visible && root.tileInViewport(latestTile, homeScroll.contentItem.contentY, homeScroll.availableHeight)
                                contextKey: root.catalogContextKey("latest", modelData.identity || modelData.title || "")
                                onClicked: if (modelData.submission) root.submissionRequested(modelData.submission)
                            }
                        }
                    }
                }

                ColumnLayout {
                    objectName: "homePopularSection"
                    visible: root.popularRows.length > 0
                    Layout.fillWidth: true
                    spacing: Theme.spacing12
                    SectionHeading {
                        text: qsTr("Most downloaded")
                        showLeading: root.catalogIncomplete
                        leadingContent: Label {
                            objectName: "homePopularQualifier"
                            text: qsTr("Among loaded mods")
                            textFormat: Text.PlainText
                            font.pointSize: Theme.small
                            color: Theme.secondary
                        }
                        Button {
                            objectName: "homePopularSeeAll"
                            text: qsTr("See all")
                            flat: true
                            Accessible.name: qsTr("See all most downloaded mods")
                            onClicked: root.browseSortRequested("downloadCount")
                        }
                    }
                    GridLayout {
                        objectName: "homePopularGrid"
                        Layout.fillWidth: true
                        columns: root.shelfColumns
                        columnSpacing: Theme.spacing12
                        rowSpacing: Theme.spacing12
                        Repeater {
                            model: root.popularRows
                            delegate: HomeModTile {
                                required property var modelData
                                required property int index
                                id: popularTile
                                objectName: "homePopularTile"
                                enterIndex: root.recentMods.length + root.latestRows.length + index
                                enterArmed: root.homeEnterArmed
                                enterGeneration: root.homeEnterGeneration
                                Layout.preferredWidth: root.cardWidth
                                Layout.maximumWidth: root.cardWidth
                                Layout.fillHeight: true
                                title: modelData.title || ""
                                subtitle: root.popularSubtitle(modelData)
                                showSubtitle: true
                                imageSource: (modelData.images || [])[0] || modelData.thumbnail || ""
                                masked: modelData.masked !== false
                                previewActive: root.visible && root.tileInViewport(popularTile, homeScroll.contentItem.contentY, homeScroll.availableHeight)
                                contextKey: root.catalogContextKey("popular", modelData.identity || modelData.title || "")
                                onClicked: if (modelData.submission) root.submissionRequested(modelData.submission)
                            }
                        }
                    }
                }

                Rectangle {
                    objectName: "homeCatalogFallback"
                    visible: !root.homeLoading && root.latestRows.length === 0 && root.popularRows.length === 0
                    Layout.fillWidth: true
                    implicitHeight: fallbackRow.implicitHeight + Theme.spacing24
                    radius: Theme.radiusCard
                    color: Theme.surfaceLow
                    border.width: 1
                    border.color: Theme.rule
                    RowLayout {
                        id: fallbackRow
                        anchors.fill: parent
                        anchors.margins: Theme.spacing12
                        anchors.leftMargin: Theme.spacing16
                        anchors.rightMargin: Theme.spacing16
                        spacing: Theme.spacing16
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacing4
                            Label {
                                Layout.fillWidth: true
                                text: root.library.demoMode ? qsTr("Explore the sample library")
                                    : root.catalogUnavailable ? qsTr("Saved catalog unavailable")
                                    : qsTr("No saved mods in your catalog yet")
                                textFormat: Text.PlainText
                                font.weight: Font.DemiBold
                                wrapMode: Text.Wrap
                            }
                            Label {
                                Layout.fillWidth: true
                                text: root.library.demoMode ? qsTr("Try searching for a mod or switching one on. Demo changes last until you close the app.")
                                    : root.catalogUnavailable ? qsTr("Open Browse to inspect or refresh the catalog.")
                                    : qsTr("Open Browse to load and explore mods from GameBanana.")
                                textFormat: Text.PlainText
                                font.pointSize: Theme.small
                                color: Theme.secondary
                                wrapMode: Text.Wrap
                            }
                        }
                        Button {
                            objectName: "homeOpenBrowseButton"
                            text: root.library.demoMode ? qsTr("Open Library") : qsTr("Open Browse")
                            onClicked: {
                                if (root.library.demoMode) root.libraryRequested()
                                else root.browseRequested()
                            }
                        }
                    }
                }
            }

            Item { Layout.preferredHeight: Theme.spacing24 }
        }
    }
}
