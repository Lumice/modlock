pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

FocusScope {
    id: root
    required property LibraryModel library
    property bool globalExternalNoticeVisible: false
    property bool fontFocusReturnAllowed: false
    property string requestedModTab: "overview"
    property string destination: "home"
    property string browsePresentationMode: "grid"
    property string libraryPresentationMode: "list"
    property string downloadedModId: ""
    property string downloadedProfileId: ""
    function selectDownloadedMod() {
        if (downloadedModId === "" || library.loading || library.viewedProfileId !== downloadedProfileId) return
        library.selectedId = downloadedModId
        if (library.selectedId === downloadedModId) { downloadedModId = ""; downloadedProfileId = "" }
    }
    signal importFilesRequested()
    signal browseRequested()
    signal downloadRequested(var submission)
    signal replacementRequested(string modId, string action)
    function focusDownload() { if (screen.status === Loader.Ready && screen.item && ("focusDownload" in screen.item)) screen.item.focusDownload() }
    function focusLibraryDownload() { if (screen.status === Loader.Ready && screen.item && ("focusDownloads" in screen.item)) screen.item.focusDownloads() }
    readonly property CatalogController catalog: library.catalog
    readonly property string catalogSourceDomain: mediaState.canonical([library.dataDir, library.sourcePath, library.nativeMode, library.demoMode])
    readonly property bool viewerOpen: screen.status === Loader.Ready && screen.item
                                       && ("viewerOpen" in screen.item) && screen.item.viewerOpen
    readonly property bool popupOpen: viewerOpen || (screen.status === Loader.Ready && screen.item
                                                     && ("popupOpen" in screen.item) && screen.item.popupOpen)
    function showHome() { if (library.activeProfileId && library.viewedProfileId !== library.activeProfileId) library.viewProfile(library.activeProfileId); destination = "home"; catalog.active = false }
    function showLibrary() { destination = "library"; catalog.active = false }
    function bulkOffForScreenshot() { if (screen.item && ("bulkOffForScreenshot" in screen.item)) screen.item.bulkOffForScreenshot() }
    function markForScreenshot() { if (screen.item && ("markForScreenshot" in screen.item)) screen.item.markForScreenshot() }
    function showSettings() { destination = "settings"; catalog.active = false }
    function showDownloads() { destination = "downloads"; catalog.active = false; library.remote.refreshDownloads() }
    function showTweaks() { destination = "tweaks"; catalog.active = false }
    // Development aid for screenshots: open a Tweaks tab.
    function tweaksReview(controller) { if (screen.item && screen.item.openReview) screen.item.openReview(controller) }
    function tweaksTab(index) { showTweaks(); if (screen.item && screen.item.tab !== undefined) screen.item.tab = index }
    function activateCatalog() { destination = "catalog"; catalog.active = true }
    function showSkins(hero) { showBrowse(); catalog.updateQuery({kind: "mod", categories: ["Skins"], heroes: hero ? [hero] : [], search: "", excludeFilters: false}) }
    function showBrowse() { catalog.browse(); activateCatalog() }
    function showBrowseSort(sort) {
        showBrowse()
        catalog.updateQuery({
            kind: "mod",
            search: "",
            categories: [],
            heroes: [],
            authorRemoteId: "",
            excludeFilters: false,
            favoritesOnly: false,
            includeWips: false,
            nsfwOnly: false,
            hideObsolete: false,
            updatedAfter: null,
            addedAfter: null,
            addedBefore: null,
            sort: sort
        })
    }
    function syncCatalogBindings() {
        if (!catalog) return
        catalog.hideNsfw = Preferences.hideNsfwMods
        catalog.homeActive = (destination === "home")
    }
    onDestinationChanged: syncCatalogBindings()
    onCatalogChanged: syncCatalogBindings()
    Connections {
        target: Preferences
        function onHideNsfwModsChanged() { root.syncCatalogBindings() }
    }
    Component.onCompleted: syncCatalogBindings()
    signal downloadsRequested()
    signal applyRequested()
    signal settingsAction(string action)
    signal importFolderRequested()
    signal upstreamImportRequested()
    signal installedRequested()
    signal artworkRequested()
    signal clearArtworkRequested()
    property url pendingArtwork
    property bool downloadsAvailable: false
    component NavigationTab: TabButton {
        id: tab
        property string iconName: ""
        property int activeDownloads: 0
        Accessible.name: activeDownloads > 0 ? qsTr("%1, %2 active downloads").arg(text).arg(activeDownloads) : text
        Layout.fillWidth: true
        enabled: !root.viewerOpen && !root.library.remote.visible
        hoverEnabled: true
        padding: Theme.spacing8
        topPadding: 10
        bottomPadding: 8
        display: AbstractButton.TextUnderIcon
        spacing: 4
        icon.source: Theme.icon(iconName)
        icon.width: 20
        icon.height: 20
        icon.color: checked ? Theme.accentInk : Theme.secondary
        font.pointSize: Theme.small
        font.weight: checked ? Font.DemiBold : Font.Medium
        Accessible.selectable: true
        Accessible.selected: checked
        Accessible.onPressAction: if (enabled && !root.viewerOpen) click()
        contentItem: Column {
            spacing: 4
            Item {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 44
                height: 30
                Item {
                    objectName: "navigationActiveHalo"
                    anchors.centerIn: parent
                    width: 30
                    height: 30
                    Repeater {
                        model: 8
                        Rectangle {
                            required property int index
                            anchors.centerIn: parent
                            width: 30 - index * 2
                            height: width
                            radius: width / 2
                            color: Qt.alpha(Theme.accent, Tokens.dark ? 0.025 : 0.018)
                            Accessible.ignored: true
                        }
                    }
                    opacity: tab.checked ? 1 : 0
                    Behavior on opacity { NumberAnimation { duration: Tokens.normal; easing.type: Easing.OutCubic } }
                    Accessible.ignored: true
                }
                ColorImage {
                    objectName: "navigationIcon"
                    anchors.centerIn: parent
                    source: tab.icon.source
                    sourceSize: Qt.size(20, 20)
                    width: 20
                    height: 20
                    color: tab.icon.color
                }
            }
            Label {
                anchors.horizontalCenter: parent.horizontalCenter
                text: tab.text
                font: tab.font
                color: tab.checked ? Theme.text : Theme.secondary
            }
            StatusPill {
                objectName: "downloadCountBadge"
                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.min(implicitWidth, parent.width)
                text: String(tab.activeDownloads)
                visible: tab.activeDownloads > 0
                Accessible.ignored: true
            }
        }
        Item {
            objectName: "navigationActiveOrnament"
            x: -Theme.spacing4
            width: 14
            height: Math.max(28, tab.height - 12)
            anchors.verticalCenter: parent.verticalCenter
            opacity: tab.checked ? 1 : 0
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Tokens.normal; easing.type: Easing.OutCubic } }
            Accessible.ignored: true
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                anchors.horizontalCenter: parent.left
                anchors.horizontalCenterOffset: 6
                width: 14
                height: 14
                radius: 7
                color: Qt.alpha(Theme.brass, Tokens.dark ? 0.22 : 0.16)
                Accessible.ignored: true
            }
            Rectangle {
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.horizontalCenter: parent.left
                anchors.horizontalCenterOffset: 6
                width: 1
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 0.5; color: Qt.alpha(Theme.brass, 0.85) }
                    GradientStop { position: 1.0; color: "transparent" }
                }
                Accessible.ignored: true
            }
            Rectangle {
                objectName: "navigationActiveDiamond"
                anchors.verticalCenter: parent.verticalCenter
                anchors.horizontalCenter: parent.left
                anchors.horizontalCenterOffset: 6
                width: 7
                height: 7
                rotation: 45
                color: Theme.brass
                Accessible.ignored: true
            }
        }
        background: Rectangle {
            color: "transparent"
            radius: Theme.radius
            border.width: tab.visualFocus ? 2 : 0
            border.color: Theme.accentInk
            Rectangle {
                objectName: "navigationHoverFill"
                anchors.fill: parent
                anchors.margins: 2
                radius: Theme.radius - 2
                color: Theme.surface
                opacity: !tab.checked && tab.hovered ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: Tokens.fast; easing.type: Easing.OutCubic } }
                Accessible.ignored: true
            }
        }
    }
    MediaViewState { id: mediaState }
    ButtonGroup { id: navigationButtons }
    Connections {
        target: root.catalog
        function onRemoteRequested() { root.activateCatalog() }
        function onReturnToLibrary() { root.showLibrary() }
    }
    Connections {
        target: root.library
        function onLoadingChanged() { root.selectDownloadedMod() }
        function onCountChanged() { root.selectDownloadedMod() }
    }
    RowLayout {
        anchors.fill: parent
        spacing: 0
        Rectangle {
            Layout.preferredWidth: 80
            Layout.fillHeight: true
            color: Theme.surfaceLow
            ColumnLayout {
                anchors.fill: parent
                anchors.topMargin: Theme.spacing12
                anchors.bottomMargin: Theme.spacing12
                anchors.leftMargin: Theme.spacing4
                anchors.rightMargin: Theme.spacing4
                spacing: Theme.spacing4
                Accessible.role: Accessible.PageTabList
                Accessible.name: qsTr("Navigation")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: Theme.spacing4
                    NavigationTab {
                        id: homeTab
                        objectName: "homeTab"
                        text: qsTr("Home")
                        iconName: "home"
                        checked: root.destination === "home"
                        ButtonGroup.group: navigationButtons
                        KeyNavigation.down: libraryTab
                        onClicked: root.showHome()
                    }
                    NavigationTab {
                        id: libraryTab
                        objectName: "libraryTab"
                        text: qsTr("Library")
                        iconName: "library"
                        checked: root.destination === "library"
                        ButtonGroup.group: navigationButtons
                        KeyNavigation.up: homeTab
                        KeyNavigation.down: browseTab
                        Accessible.description: qsTr("Your mods in the viewed profile")
                        onClicked: if (!root.viewerOpen) root.showLibrary()
                    }
                    NavigationTab {
                        id: browseTab
                        objectName: "browseTab"
                        text: qsTr("Browse")
                        iconName: "compass"
                        checked: root.destination === "catalog"
                        ButtonGroup.group: navigationButtons
                        KeyNavigation.up: libraryTab
                        KeyNavigation.down: downloadsTab
                        Accessible.description: qsTr("Find mods on GameBanana")
                        onClicked: if (!root.viewerOpen) root.showBrowse()
                    }
                    NavigationTab {
                        id: downloadsTab
                        activeDownloads: root.library.remote.jobs.filter(job => !["installed", "failed", "cancelled", "abandoned"].includes(job.state)).length
                        objectName: "downloadsTab"
                        text: qsTr("Downloads")
                        iconName: "download"
                        checked: root.destination === "downloads"
                        ButtonGroup.group: navigationButtons
                        visible: root.library.nativeMode
                        enabled: root.downloadsAvailable && !root.viewerOpen && !root.library.remote.visible
                        KeyNavigation.up: browseTab
                        KeyNavigation.down: tweaksTab
                        Accessible.description: qsTr("Download progress and history")
                        onClicked: root.showDownloads()
                    }
                    NavigationTab {
                        id: tweaksTab
                        objectName: "tweaksTab"
                        text: qsTr("Tweaks")
                        iconName: "sliders"
                        visible: root.library.nativeMode
                        checked: root.destination === "tweaks"
                        ButtonGroup.group: navigationButtons
                        KeyNavigation.up: downloadsTab
                        KeyNavigation.down: settingsTab
                        Accessible.description: qsTr("Performance configs and other game settings")
                        onClicked: if (!root.viewerOpen) root.showTweaks()
                    }
                }
                Item { Layout.fillHeight: true }
                NavigationTab {
                    id: settingsTab
                    objectName: "settingsTab"
                    text: qsTr("Settings")
                    iconName: "settings"
                    checked: root.destination === "settings"
                    ButtonGroup.group: navigationButtons
                    KeyNavigation.up: tweaksTab
                    Accessible.description: qsTr("Game folder, library, fonts and app info")
                    onClicked: if (!root.viewerOpen) root.showSettings()
                }
            }
        }
        Rectangle { Layout.fillHeight: true; Layout.preferredWidth: 1; color: Theme.rule }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Frame {
                Layout.fillWidth: true
                visible: !root.library.loading && !root.library.demoMode && !root.library.gamePath && !root.library.detectedGamePath && root.destination !== "settings"
                RowLayout {
                    anchors.fill: parent
                    Label { Layout.fillWidth: true; text: qsTr("Deadlock wasn't found. You can collect mods now, then choose its folder in Settings when it's installed."); wrapMode: Text.Wrap }
                    Button { objectName: "firstRunGameSettings"; text: qsTr("Choose game folder…"); onClicked: root.showSettings() }
                    Button { visible: !root.library.nativeMode && root.destination !== "home" && root.destination !== "library"; text: qsTr("Create library"); enabled: root.library.canCreateLibrary; onClicked: root.library.createLibrary() }
                }
            }
            Loader {
                id: screen
                Layout.fillWidth: true
                Layout.fillHeight: true
                active: true
                enabled: !root.library.remote.visible
                sourceComponent: root.destination === "home" ? homePage : root.destination === "settings" ? settingsPage : root.destination === "tweaks" ? tweaksPage : root.destination === "downloads" ? downloadsPage : root.destination === "library" ? libraryPage : root.catalog.route === "submission" ? submissionPage : root.catalog.route === "collection" ? collectionPage : browsePage
            }
        }
    }
    DownloadBubbles {
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: Theme.spacing24
        anchors.rightMargin: root.destination === "library" && screen.width >= 926 && !!root.library.selectedId ? 330 + Theme.spacing24 : Theme.spacing24
        library: root.library
        onLibraryRequested: (modId, profileId) => {
            root.library.query = ""
            root.library.category = ""
            root.library.heroFilter = ""
            root.library.typeFilter = ""
            root.library.enabledOnly = false
            root.library.needsAttentionOnly = false
            root.library.updatesOnly = false
            root.showLibrary()
            root.downloadedModId = modId
            root.downloadedProfileId = profileId
            if (root.library.viewedProfileId !== profileId) root.library.viewProfile(profileId)
            root.selectDownloadedMod()
        }
    }
    Component {
        id: homePage
        HomePage {
            globalExternalNoticeVisible: root.globalExternalNoticeVisible
            library: root.library
            catalog: root.catalog
            viewState: mediaState
            onLibraryRequested: root.showLibrary()
            onApplyRequested: root.applyRequested()
            onInstalledRequested: root.installedRequested()
            onBrowseRequested: root.showBrowse()
            onBrowseSortRequested: sort => root.showBrowseSort(sort)
            onSubmissionRequested: submission => { root.requestedModTab = "overview"; root.catalog.openSubmission(submission, false) }
        }
    }
    Component {
        id: libraryPage
        LibraryPage {
            library: root.library
            viewState: mediaState
            fontFocusReturnAllowed: root.fontFocusReturnAllowed && root.destination === "library" && !root.viewerOpen
            presentationMode: root.libraryPresentationMode
            onPresentationModeRequested: mode => root.libraryPresentationMode = mode === "grid" ? "grid" : "list"
            onRemoteRequested: submission => { root.requestedModTab = "overview"; root.catalog.openSubmission(submission, true) }
            onChangelogRequested: submission => { root.requestedModTab = "changelog"; root.catalog.openSubmission(submission, true) }
            onAuthorRequested: authorId => root.catalog.author(authorId)
            onReplacementRequested: (modId, action) => root.replacementRequested(modId, action)
            onImportFilesRequested: root.importFilesRequested()
            onBrowseRequested: root.browseRequested()
            onApplyRequested: root.applyRequested()
            onImportFolderRequested: root.importFolderRequested()
            onUpstreamImportRequested: root.upstreamImportRequested()
            onInstalledRequested: root.installedRequested()
            onArtworkRequested: root.artworkRequested()
            onClearArtworkRequested: root.clearArtworkRequested()
            pendingArtwork: root.pendingArtwork
        }
    }
    Component {
        id: browsePage
        BrowsePage {
            controller: root.catalog
            viewState: mediaState
            demoMode: root.library.demoMode
            sourceDomain: root.catalogSourceDomain
            presentationMode: root.browsePresentationMode
            installAvailable: root.library.remote.canResolve
            onInstallRequested: submission => root.downloadRequested(submission)
            onPresentationModeRequested: mode => root.browsePresentationMode = mode === "grid" ? "grid" : "list"
        }
    }
    Component { id: submissionPage; CatalogDetails { Component.onCompleted: { if (root.requestedModTab === "changelog") Qt.callLater(() => openTab("changelog")); root.requestedModTab = "overview" } controller: root.catalog; remote: root.library.remote; onDownloadRequested: submission => root.downloadRequested(submission) } }
    Component {
        id: downloadsPage
        Item {
          readonly property bool popupOpen: downloadHistory.popupOpen
          ColumnLayout {
            anchors.fill: parent
            anchors.margins: Theme.spacing24
            SectionHeading { text: qsTr("Downloads"); pointSize: Theme.heading }
            Label { Layout.fillWidth: true; visible: root.library.remote.error !== ""; text: Theme.downloadError(root.library.remote.error); textFormat: Text.PlainText; wrapMode: Text.Wrap }
            Item {
                Layout.fillWidth: true; Layout.fillHeight: true
                DownloadHistory { id: downloadHistory; anchors.fill: parent; library: root.library; onBrowseRequested: root.showBrowse(); onCloseRequested: root.showLibrary() }
            }
          }
        }
    }
    Component {
        id: tweaksPage
        TweaksPage { library: root.library; onApplyRequested: root.applyRequested() }
    }
    Component {
        id: settingsPage
        SettingsPage {
            library: root.library
            onLaunchSettingsRequested: root.settingsAction("launchSettings")
            onApplyRequested: root.settingsAction("apply")
            onRestoreRequested: root.settingsAction("restore")
            onOrderRequested: root.settingsAction("order")
            onHistoryRequested: root.settingsAction("history")
            onSourceRequested: root.settingsAction("source")
            onProfilesRequested: root.settingsAction("profiles")
            onInstalledRequested: root.installedRequested()
            onBisectRequested: root.settingsAction("bisect")
        }
    }
    Component { id: collectionPage; CollectionDetails { controller: root.catalog; installAvailable: root.library.remote.canResolve; onDownloadRequested: submission => root.downloadRequested(submission) } }
    Shortcut { sequences: [StandardKey.Back]; enabled: root.destination === "catalog" && root.catalog.canBack && !root.viewerOpen && !root.library.remote.visible; onActivated: root.catalog.back() }
    Keys.onEscapePressed: event => {
        if (root.viewerOpen && screen.status === Loader.Ready) {
            screen.item.closeViewer()
            event.accepted = true
        } else if (!root.library.remote.visible && root.destination === "catalog" && root.catalog.canBack) {
            root.catalog.back()
            event.accepted = true
        }
    }
}
