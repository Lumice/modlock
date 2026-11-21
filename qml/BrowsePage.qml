pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import "HeroData.js" as HeroData

FocusScope {
    id: root
    required property CatalogController controller
    Binding { target: root.controller; property: "hideNsfw"; value: Preferences.hideNsfwMods }
    required property MediaViewState viewState
    required property string sourceDomain
    property string presentationMode: "list"
    signal presentationModeRequested(string mode)
    signal installRequested(var submission)
    property string heroSearch: ""
    property bool demoMode: false
    property bool installAvailable: false
    function compactCount(value) {
        const number = Number(value)
        return number >= 1000000 ? qsTr("%1m").arg((number / 1000000).toFixed(1).replace(/\.0$/, ""))
             : number >= 1000 ? qsTr("%1k").arg((number / 1000).toFixed(1).replace(/\.0$/, ""))
             : root.controller.displayCount(value)
    }
    function actionText(action) {
        switch (action.action) {
        case "enable": return qsTr("Enable")
        case "disable": return qsTr("Disable")
        case "update": return qsTr("Update")
        case "progress": return action.percent >= 0 ? qsTr("Downloading %1%").arg(action.percent) : qsTr("Downloading…")
        case "ambiguous": return qsTr("Multiple matches")
        case "unavailable": return qsTr("Unavailable")
        default: return qsTr("Download")
        }
    }
    readonly property var query: controller.queryState
    readonly property var heroChoices: HeroData.heroes.concat(root.controller.heroes.filter(hero => HeroData.heroes.indexOf(hero) < 0))
    readonly property bool skinsMode: (query.categories || []).indexOf("Skins") >= 0 && query.kind === "mod"
    readonly property bool heroGridVisible: skinsMode && !(query.heroes || []).length
    function showHero(hero) { controller.updateQuery({kind: "mod", categories: ["Skins"], heroes: [hero], search: "", excludeFilters: false}) }
    readonly property bool advanced: controller.advanced
    readonly property int secondaryCount: [query.favoritesOnly, query.hideNsfw, query.nsfwOnly, query.hideObsolete,
                                           query.includeWips, query.excludeFilters, query.authorRemoteId,
                                           query.updatedAfter, query.addedAfter, query.addedBefore].filter(Boolean).length
    readonly property bool secondaryChanged: secondaryCount > (query.hideNsfw ? 1 : 0) || !query.hideNsfw
    function resetSecondaryFilters() {
        Preferences.hideNsfwMods = true
        controller.updateQuery({favoritesOnly: false, hideNsfw: true, nsfwOnly: false, hideObsolete: false,
                                includeWips: false, excludeFilters: false, authorRemoteId: "",
                                updatedAfter: null, addedAfter: null, addedBefore: null})
    }
    property var generation: 0
    property string selectedIdentity: controller.selectedIdentity
    property string rememberedFocus: controller.focusKey
    readonly property bool viewerOpen: results.viewerOpen
    readonly property bool popupOpen: sourceMenu.visible || kinds.down || categories.down || heroes.down || sorting.down || filterPopup.opened
                                      || (filterLoader.status === Loader.Ready && filterLoader.item && filterLoader.item.popupOpen)
    component FilterCombo: ComboBox {
        id: filter
        Layout.preferredWidth: Math.min(Math.max(120, root.width * 0.26), Math.max(100, filterMetrics.advanceWidth + leftPadding + rightPadding + (indicator ? indicator.width : 0) + spacing))
        Layout.minimumWidth: 100
        TextMetrics { id: filterMetrics; font: filter.font; text: filter.displayText }
        contentItem: Label {
            text: filter.displayText
            textFormat: Text.PlainText
            font: filter.font
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
        }
        delegate: ItemDelegate {
            id: option
            required property var modelData
            required property int index
            readonly property string valueText: filter.textRole ? String(modelData[filter.textRole] || "") : String(modelData)
            width: filter.width
            highlighted: filter.highlightedIndex === index
            contentItem: Label { text: option.valueText; textFormat: Text.PlainText; font: option.font; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
        }
    }
    function closeViewer() { results.closeViewer() }
    function remember(identity, position, focus) {
        if (generation !== controller.viewGeneration) return
        selectedIdentity = identity
        rememberedFocus = focus
        controller.remember(identity, position, focus, generation)
    }
    function setPresentation(mode) {
        if (mode === presentationMode) return
        results.beforeModeChange()
        presentationModeRequested(mode)
    }
    function snapshotFor(identity, revision) {
        const row = controller.model.item(identity)
        if (!row || row.identity !== identity || row.state !== "ready") return null
        const metadata = row.metadata || {}
        const data = row.collection ? (metadata.header || {}) : metadata
        const source = String(row.collection ? data.coverUrl || "" : data.thumbnail || "")
        const images = data.images || []
        const sources = []
        for (let i = 0; i < images.length; ++i) sources.push(String(images[i]))
        return {identity: identity, source: source, sources: sources,
                hidden: row.collection ? data.isNsfw === undefined || !!data.isNsfw || !data.contentRatingKnown
                                       : data.masked === undefined || !!data.masked,
                title: String(row.collection ? data.name || "" : data.title || "")}
    }
    function openDetails(identity, index) {
        results.selectIndex(index, identity)
        results.capture()
        controller.openItem(identity)
    }
    // One sound preview plays at a time. Anything that changes what this page
    // owns stops it and forgets owner-specific mature reveals.
    readonly property var audioPlayer: typeof audioPreview !== "undefined" ? audioPreview : null
    readonly property string audioContext: viewState.canonical([sourceDomain, query, controller.page, controller.viewGeneration,
                                                                controller.route, controller.active, Preferences.showMaturePreviews])
    onAudioContextChanged: if (audioPlayer) audioPlayer.invalidate()
    Connections {
        target: root.controller.model
        function onModelReset() { if (root.audioPlayer) root.audioPlayer.invalidate() }
    }
    Component.onDestruction: if (audioPlayer) audioPlayer.invalidate()
    Component.onCompleted: {
        generation = controller.viewGeneration
        selectedIdentity = controller.selectedIdentity
        rememberedFocus = controller.focusKey
    }
    Connections {
        target: root.controller
        function onChanged() {
            if (root.generation !== root.controller.viewGeneration) {
                root.generation = root.controller.viewGeneration
                root.selectedIdentity = root.controller.selectedIdentity
                root.rememberedFocus = root.controller.focusKey
                search.text = root.heroGridVisible ? root.heroSearch : root.query.search
            }
        }
    }
    ButtonGroup { id: presentationButtons }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 24
        spacing: Theme.spacing12
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            Button {
                visible: root.controller.canBack
                text: qsTr("Back")
                icon.source: Theme.icon("arrow-left")
                flat: true
                onClicked: root.controller.back()
            }
            Label { text: qsTr("Browse"); font.family: Theme.displayFamily; font.pointSize: Theme.heading; font.weight: Font.DemiBold; Layout.rightMargin: Theme.spacing8 }
            ToolButton {
                text: qsTr("Refresh")
                icon.source: Theme.icon("refresh")
                display: AbstractButton.IconOnly
                enabled: root.controller.available && !root.controller.busy
                ToolTip.visible: hovered || visualFocus
                ToolTip.text: text
                onClicked: root.controller.refresh()
            }
            ToolButton {
                text: qsTr("More")
                icon.source: Theme.icon("more")
                display: AbstractButton.IconOnly
                Accessible.name: qsTr("Browse actions")
                onClicked: sourceMenu.popup()
                Menu {
                    id: sourceMenu
                    MenuItem { text: qsTr("Catalog source info"); checkable: true; checked: catalogStatus.provenanceExpanded; onTriggered: catalogStatus.provenanceExpanded = !catalogStatus.provenanceExpanded }
                }
            }
            TextField {
                id: search
                enabled: root.heroGridVisible || root.controller.available
                Layout.fillWidth: true
                Layout.minimumWidth: 120
                leftPadding: 36
                text: root.heroGridVisible ? root.heroSearch : root.query.search
                placeholderText: root.heroGridVisible ? qsTr("Find a hero") : root.query.kind === "collections" ? qsTr("Search collections or authors") : root.query.kind === "sound" ? qsTr("Search sounds") : qsTr("Search mods")
                Accessible.name: placeholderText
                onTextEdited: { if (root.heroGridVisible) root.heroSearch = text; else debounce.restart() }
                onActiveFocusChanged: if (activeFocus && !results.restoring) {
                    root.remember(root.selectedIdentity, results.view ? results.view.contentY : 0, "search")
                    results.queueCapture()
                }
                Keys.onEscapePressed: { text = ""; debounce.stop(); if (root.heroGridVisible) root.heroSearch = ""; else root.controller.updateQuery({search: ""}) }
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
            Timer { id: debounce; interval: 300; onTriggered: root.controller.updateQuery({search: search.text}) }
        }
        Flow {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            SegmentedControl {
                currentValue: root.skinsMode ? "skins" : root.query.kind
                model: [{text: qsTr("Mods"), value: "mod", objectName: "browseModsButton"}, {text: qsTr("Hero skins"), value: "skins", objectName: "browseSkinsButton"}, {text: qsTr("Sounds"), value: "sound", objectName: "browseSoundsButton"}, {text: qsTr("Collections"), value: "collections", objectName: "browseCollectionsButton"}]
                onActivated: value => {
                    if (value === currentValue) return
                    // Sounds keep the kind query the former More entry used.
                    root.controller.updateQuery({kind: value === "skins" ? "mod" : value,
                        categories: value === "skins" ? ["Skins"] : [], heroes: value === "collections" || value === "sound" ? [] : root.query.heroes})
                }
            }
            FilterCombo {
                id: kinds
                enabled: root.controller.available
                model: [{name: qsTr("WIPs"), value: "wip"}, {name: qsTr("Maps"), value: "maps"}]
                textRole: "name"
                valueRole: "value"
                displayText: currentIndex < 0 ? qsTr("More") : currentText
                currentIndex: root.query.kind === "" ? -1 : model.findIndex(item => item.value === root.query.kind)
                Accessible.name: qsTr("Catalog kind")
                onActivated: root.controller.updateQuery({kind: currentValue, categories: [], heroes: []})
            }
        }
        Flow {
            objectName: "browseFilterFlow"
            visible: !root.heroGridVisible
            Layout.fillWidth: true
            spacing: Theme.spacing8
            enabled: root.controller.available
            FilterCombo {
                id: heroes
                objectName: "browseHeroFilter"
                visible: root.query.kind !== "collections"
                editable: true
                model: [qsTr("All heroes")].concat(root.heroChoices)
                currentIndex: (root.query.heroes || []).length ? root.heroChoices.indexOf(root.query.heroes[0]) + 1 : 0
                Accessible.name: qsTr("Catalog hero")
                onActivated: root.controller.updateQuery({heroes: currentIndex ? [root.heroChoices[currentIndex - 1]] : []})
                onAccepted: {
                    const index = find(editText, Qt.MatchFixedString)
                    if (index >= 0) root.controller.updateQuery({heroes: index ? [root.heroChoices[index - 1]] : []})
                    else editText = currentText
                }
                contentItem: TextField {
                    text: heroes.editText
                    enabled: heroes.editable
                    selectByMouse: true
                    leftPadding: 0
                    rightPadding: 0
                    verticalAlignment: Text.AlignVCenter
                    background: null
                }
            }
            FilterCombo {
                id: categories
                visible: root.query.kind !== "collections"
                model: [qsTr("All types")].concat(root.controller.categories)
                currentIndex: (root.query.categories || []).length ? root.controller.categories.indexOf(root.query.categories[0]) + 1 : 0
                Accessible.name: qsTr("Catalog type")
                onActivated: root.controller.updateQuery({categories: currentIndex ? [root.controller.categories[currentIndex - 1]] : []})
            }
            FilterCombo {
                id: sorting
                visible: root.query.kind !== "collections"
                model: [{name: qsTr("Default order"), value: "default"}, {name: qsTr("Last updated"), value: "lastUpdated"}, {name: qsTr("Most downloaded"), value: "downloadCount"}, {name: qsTr("Most liked"), value: "rating"}, {name: qsTr("Newest"), value: "releaseDate"}]
                textRole: "name"
                valueRole: "value"
                currentIndex: model.findIndex(item => item.value === root.query.sort)
                Accessible.name: qsTr("Catalog sort order")
                onActivated: root.controller.updateQuery({sort: currentValue})
            }
            Rectangle { width: 1; height: 28; color: Theme.rule }
            FilterChip {
                objectName: "browseFavoritesQuick"
                text: qsTr("Favorites")
                checked: root.query.favoritesOnly
                onToggled: root.controller.updateQuery({favoritesOnly: checked})
            }
            FilterChip {
                id: moreFilters
                objectName: "browseMoreFilters"
                text: qsTr("Filters")
                count: root.secondaryCount
                icon.source: Theme.icon("sliders")
                checkable: true
                checked: filterPopup.opened
                onClicked: filterPopup.opened ? filterPopup.close() : filterPopup.open()
            }
            Button {
                objectName: "browseResetFilters"
                visible: root.secondaryChanged
                text: qsTr("Reset extra filters")
                flat: true
                onClicked: root.resetSecondaryFilters()
            }
        }
        Popup {
            id: filterPopup
            objectName: "browseFilterPopup"
            parent: root
            x: Math.max(0, Math.min(root.width - width, moreFilters.mapToItem(root, 0, 0).x))
            y: Math.max(0, Math.min(root.height - height, moreFilters.mapToItem(root, 0, moreFilters.height).y))
            width: Math.min(460, root.width)
            height: Math.min(filterContent.implicitHeight + topPadding + bottomPadding, root.height - 24)
            padding: Theme.spacing12
            closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
            ScrollView {
                anchors.fill: parent
                contentWidth: availableWidth
                clip: true
                ScrollBar.vertical.policy: filterContent.implicitHeight > availableHeight ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff
                ColumnLayout {
                    id: filterContent
                    width: parent.width
                    CheckBox {
                        objectName: "browseMatureResults"
                        text: qsTr("Hide mature results")
                        checked: root.query.hideNsfw
                        Accessible.description: qsTr("Hide sensitive or unrated results")
                        onToggled: Preferences.hideNsfwMods = checked
                    }
                    Loader {
                        id: filterLoader
                        Layout.fillWidth: true
                        active: filterPopup.opened && root.query.kind !== "collections"
                        sourceComponent: CatalogFilters { controller: root.controller }
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.query.kind !== "collections" && root.controller.searchPrerequisite !== ""
            text: root.controller.searchPrerequisite
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        RowLayout {
            Layout.fillWidth: true
            visible: !!root.query.authorRemoteId
            StatusPill { text: qsTr("Author %1").arg(root.query.authorRemoteId || ""); tone: "accent" }
            Button { text: qsTr("Clear"); flat: true; icon.source: Theme.icon("x"); onClicked: root.controller.updateQuery({authorRemoteId: ""}) }
            Item { Layout.fillWidth: true }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing12
            visible: !root.heroGridVisible
            CatalogStatus { id: catalogStatus; headerActions: true; Layout.fillWidth: true; controller: root.controller; demoMode: root.demoMode }
            Item { Layout.fillWidth: true; visible: !catalogStatus.hasNotice }
            Label { visible: root.controller.contentReady && /^\d+$/.test(root.controller.total); text: qsTr("%1 results").arg(Number(root.controller.total).toLocaleString()); color: Theme.secondary }
            Rectangle {
                Layout.alignment: Qt.AlignTop
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
                        onClicked: root.setPresentation("list")
                    }
                    ToolButton {
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
                        onClicked: root.setPresentation("grid")
                    }
                }
            }
        }
        RowLayout {
            visible: root.skinsMode
            Layout.fillWidth: true
            Button {
                visible: !root.heroGridVisible
                objectName: "allSkinHeroesButton"
                text: qsTr("All heroes")
                icon.source: Theme.icon("arrow-left")
                onClicked: root.controller.updateQuery({heroes: [], search: ""})
            }
            Label {
                Layout.fillWidth: true
                text: root.heroGridVisible ? qsTr("Choose a hero") : qsTr("Skins for %1").arg((root.query.heroes || [])[0] || "")
                textFormat: Text.PlainText
                font.pointSize: Theme.heading
            }
        }
        HeroGrid {
            visible: root.heroGridVisible
            Layout.fillWidth: true
            Layout.fillHeight: true
            artwork: root.controller.heroArtwork
            search: root.heroSearch
            onHeroSelected: hero => root.showHero(hero)
        }
        MediaResultView {
            id: results
            objectName: "catalogSkinResults"
            visible: !root.heroGridVisible
            Layout.fillWidth: true
            Layout.fillHeight: true
            stateBank: root.viewState
            model: root.controller.model
            scope: "browse"
            domain: root.sourceDomain
            contextKey: root.viewState.canonical([root.sourceDomain, root.query, root.controller.page])
            selectedIdentity: root.selectedIdentity
            focusKey: root.rememberedFocus
            presentationMode: root.presentationMode
            ownerEpoch: root.controller.viewGeneration
            initialScroll: root.controller.scrollPosition
            ready: root.controller.active && root.controller.route === "browse" && root.controller.contentReady
            indexForIdentity: identity => root.controller.model.indexOf(identity)
            identityAt: index => ""
            snapshotFor: (identity, revision) => root.snapshotFor(identity, revision)
            delegateComponent: Component {
                CatalogRow {
                    id: entry
                    readonly property var libraryAction: { const revision = root.controller.libraryActionRevision; return root.controller.libraryAction(submission || {}) }
                    resultHost: results
                    ownerSnapshot: root.snapshotFor(identity, results.modelRevision)
                    selected: root.selectedIdentity === identity || (results.view && results.view.currentIndex === index)
                    favoriteBusy: root.controller.favoriteBusy
                    installVisible: !collection
                    installEnabled: !!libraryAction.enabled
                    installText: root.actionText(libraryAction)
                    installUpdate: libraryAction.action === "update"
                    installedToggle: ["enable", "disable"].includes(libraryAction.action)
                    installedOn: libraryAction.action === "disable"
                    installedProfile: libraryAction.profileName || ""
                    statsText: {
                        const data = metadata || {}
                        const parts = []
                        if (data.downloads !== null && data.downloads !== undefined) parts.push(qsTr("%1 downloads").arg(root.controller.displayCount(data.downloads)))
                        if (root.presentationMode === "grid" && data.likes !== null && data.likes !== undefined) parts.push(qsTr("%1 likes").arg(root.controller.displayCount(data.likes)))
                        return parts.join(", ")
                    }
                    catalogStats: {
                        const data = metadata || {}
                        return [{icon: "download", value: data.downloads}]
                            .concat(root.presentationMode === "grid" ? [{icon: "heart", value: data.likes}] : [])
                            .filter(stat => stat.value !== null && stat.value !== undefined)
                            .map(stat => ({icon: stat.icon, text: root.compactCount(stat.value)}))
                    }
                    onInstallRequested: {
                        const action = root.controller.libraryAction(submission || {})
                        if (action.action === "download" && action.enabled) root.installRequested(submission)
                        else root.controller.runLibraryAction(submission || {})
                    }
                    onDetailsRequested: root.openDetails(identity, index)
                    onActionRequested: root.controller.setFavorite(submission, !favorite)
                }
            }
            onSelectionRequested: identity => root.remember(identity, results.view ? results.view.contentY : 0, "results")
            onDetailsRequested: identity => root.openDetails(identity, root.controller.model.indexOf(identity))
            onPositionChanged: (position, focus) => root.remember(root.selectedIdentity, position, focus)
            onSearchFocusRequested: search.forceActiveFocus()
            EmptyState {
                anchors.centerIn: parent
                width: parent.width
                visible: results.count === 0 && !root.controller.busy && (root.controller.contentReady || root.controller.error !== "")
                headline: !root.controller.available ? root.controller.prerequisite
                    : root.controller.error !== "" ? qsTr("No saved results here yet. Refresh to load them from GameBanana.")
                    : qsTr("Nothing matches these filters")
                text: root.controller.available && root.controller.error === "" ? qsTr("Try different filters or refresh the catalog.") : ""
            }
        }
        Rectangle {
            visible: !root.heroGridVisible
            Layout.fillWidth: true
            implicitHeight: pagination.implicitHeight + 8
            color: Theme.surfaceLow
            Rectangle { anchors.top: parent.top; width: parent.width; height: 1; color: Theme.rule }
            RowLayout {
                id: pagination
                anchors.fill: parent
                anchors.margins: 4
                Button { text: qsTr("Previous"); flat: true; icon.source: Theme.icon("chevron-left"); enabled: root.controller.page > 0; onClicked: root.controller.nextPage(-1) }
                Label { Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter; color: Theme.secondary; text: root.controller.contentReady ? (Number(root.controller.total) > 0 ? qsTr("Page %1 of %2").arg(Number(root.controller.page + 1).toLocaleString()).arg(Math.ceil(Number(root.controller.total) / (root.query.kind === "collections" ? 50 : root.query.pageSize || 24)).toLocaleString()) : qsTr("Page %1").arg(Number(root.controller.page + 1).toLocaleString())) : ""; textFormat: Text.PlainText }
                Button { text: qsTr("Next"); flat: true; icon.source: Theme.icon("chevron-right"); LayoutMirroring.enabled: true; enabled: root.controller.hasNext; onClicked: root.controller.nextPage(1) }
            }
        }
    }
    Shortcut { sequences: [StandardKey.Find]; onActivated: search.forceActiveFocus() }
}
