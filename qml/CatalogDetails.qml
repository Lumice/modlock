pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

FocusScope {
    id: root
    required property CatalogController controller
    required property RemoteController remote
    signal downloadRequested(var submission)
    function focusDownload() { if (installedSwitch.visible && installedSwitch.enabled && !viewerOpen) installedSwitch.forceActiveFocus(); else if (downloadButton.visible && downloadButton.enabled && !viewerOpen) downloadButton.forceActiveFocus() }
    readonly property bool popupOpen: detailMenu.visible
    readonly property var detail: controller.detail
    readonly property var summary: detail.summary || {}
    readonly property var libraryAction: { const revision = controller.libraryActionRevision; return controller.libraryAction(summary.submission || {}) }
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
    readonly property var images: summary.images || []
    readonly property var sources: images.length ? images : summary.thumbnail ? [summary.thumbnail] : []
    readonly property bool imageHidden: summary.masked === undefined || summary.masked
    readonly property string mediaContext: JSON.stringify([summary.identity || "", controller.viewGeneration])
    readonly property bool mediaAvailable: visible && controller.active && controller.route === "submission"
                                           && detail.state !== "unavailable" && !!summary.identity && sources.length > 0
    property int galleryIndex: 0
    property var generation: 0
    property bool restoring: true
    readonly property bool viewerOpen: viewer.opened
    readonly property var supportLinks: uniqueSupportLinks((detail.metadata || {}).donationLinks || [])
    readonly property var descriptionLinks: uniqueDescriptionLinks(detail.links || [], supportLinks)

    function uniqueSupportLinks(items) {
        const seen = new Set()
        return items.filter(link => {
            const url = String(link.url || "")
            if (!url || seen.has(url)) return false
            seen.add(url)
            return true
        })
    }
    function uniqueDescriptionLinks(items, support) {
        const seen = new Set(support.map(link => String(link.url)))
        return items.filter(link => {
            const url = String(link.url || "")
            if (!url || seen.has(url)) return false
            seen.add(url)
            return true
        })
    }
    function shortDate(value) {
        return value === null || value === undefined ? qsTr("Unknown")
             : new Date(Number(value) * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }
    function rememberFocus(key) { if (!root.restoring) root.controller.remember(root.controller.selectedIdentity, scroller.contentItem.contentY, key, root.generation) }
    function restore() {
        if (!root.controller.contentReady) return
        root.restoring = true
        scroller.contentItem.contentY = Math.max(0, root.controller.scrollPosition)
        const target = {author: authorButton, favorite: favoriteButton, provider: providerButton, files: filesButton, comments: commentsButton, changelog: changelogButton}[root.controller.focusKey] || back
        if (target.visible && target.enabled) target.forceActiveFocus()
        else back.forceActiveFocus()
        Qt.callLater(() => root.restoring = false)
    }
    function openViewer(invoker) {
        if (!mediaAvailable) return
        viewer.open({sources: sources, token: mediaContext, hidden: imageHidden,
                     source: String(sources[galleryIndex] || ""), startIndex: galleryIndex}, preview, invoker)
    }
    function closeViewer() { viewer.close() }
    onGalleryIndexChanged: if (preview) preview.revealed = false
    Component.onCompleted: { generation = controller.viewGeneration; Qt.callLater(restore) }
    Connections {
        target: root.controller
        function onChanged() {
            if (root.generation !== root.controller.viewGeneration) {
                root.generation = root.controller.viewGeneration
                root.galleryIndex = 0
                root.tab = "overview"
                root.restoring = true
                Qt.callLater(root.restore)
            } else {
                if (root.galleryIndex >= root.sources.length) root.galleryIndex = 0
                if (root.restoring && root.controller.contentReady) Qt.callLater(root.restore)
            }
        }
    }
    ViewerSession {
        id: viewer
        valid: !!snapshot && root.mediaAvailable && snapshot.token === root.mediaContext
               && snapshot.hidden === root.imageHidden && viewer.sameSources(snapshot.sources, root.sources)
        title: root.summary.title || qsTr("Mod preview")
        onFocusRequested: (state, owner, target, index) => {
            if (!root.visible || !root.controller.active || root.controller.route !== "submission") return
            if (target && target.visible && target.enabled) target.forceActiveFocus()
            else if (back.visible && back.enabled) back.forceActiveFocus()
            else root.forceActiveFocus()
        }
        onIndexSelected: index => root.galleryIndex = index
        onImageRevealed: (index, source, contextKey) => {
            if (contextKey === root.mediaContext && index === root.galleryIndex
                    && String(root.sources[index] || "") === source) preview.revealed = true
        }
    }
    readonly property bool wide: width >= 900
    // "overview" shows the description; the other tabs load a provider resource.
    property string tab: "overview"
    readonly property bool updatedRecently: !!root.summary.updatedAt && (Date.now() / 1000 - Number(root.summary.updatedAt)) < 14 * 86400
    function relativeDate(value) {
        if (value === null || value === undefined || value === "") return qsTr("Unknown")
        const days = Math.floor((Date.now() / 1000 - Number(value)) / 86400)
        if (days < 1) return qsTr("Today")
        if (days < 2) return qsTr("Yesterday")
        if (days < 30) return qsTr("%1 days ago").arg(days)
        if (days < 60) return qsTr("1 month ago")
        if (days < 365) return qsTr("%1 months ago").arg(Math.floor(days / 30))
        const years = Math.floor(days / 365)
        return years === 1 ? qsTr("1 year ago") : qsTr("%1 years ago").arg(years)
    }
    function openTab(kind) {
        root.tab = kind
        if (kind !== "overview") root.controller.loadResource(kind)
    }
    component OverlayButton: ToolButton {
        implicitWidth: 32
        implicitHeight: 32
        display: AbstractButton.IconOnly
        icon.color: Theme.text
        opacity: enabled ? 1 : 0.4
        background: Rectangle {
            anchors.fill: parent; anchors.margins: -6
            radius: Theme.radius; color: Theme.surface
            Rectangle { anchors.fill: parent; anchors.margins: 2; radius: parent.radius; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: parent.parent.visualFocus }
        }
    }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 24
        spacing: Theme.spacing12
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            Button { id: back; text: qsTr("Back"); flat: true; icon.source: Theme.icon("arrow-left"); enabled: root.controller.canBack; onClicked: root.controller.back() }
            Item { Layout.fillWidth: true }
            ToolButton {
                text: qsTr("Refresh"); icon.source: Theme.icon("refresh"); display: AbstractButton.IconOnly
                enabled: root.controller.available && !root.controller.busy
                ToolTip.visible: hovered || visualFocus; ToolTip.text: text
                onClicked: root.controller.refresh()
            }
            ToolButton {
                text: qsTr("More"); icon.source: Theme.icon("more"); display: AbstractButton.IconOnly
                Accessible.name: qsTr("Mod page actions")
                onClicked: detailMenu.popup()
                Menu { id: detailMenu; MenuItem { text: qsTr("Catalog source info"); checkable: true; checked: detailStatus.provenanceExpanded; onTriggered: detailStatus.provenanceExpanded = !detailStatus.provenanceExpanded } }
            }

        }
        ScrollView {
            id: scroller
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            Connections {
                target: scroller.contentItem
                function onContentYChanged() { if (!root.restoring) root.controller.remember(root.controller.selectedIdentity, scroller.contentItem.contentY, root.controller.focusKey, root.generation) }
            }
            ColumnLayout {
                width: Math.min(scroller.availableWidth, 1100)
                x: Math.max(0, (scroller.availableWidth - width) / 2)
                spacing: Theme.spacing16
                CatalogStatus { id: detailStatus; headerActions: true; Layout.fillWidth: true; controller: root.controller; compactProvenance: true }
                Label {
                    Layout.fillWidth: true
                    visible: root.detail.state === "missing" || root.detail.state === "unavailable" || root.detail.state === "partial"
                    text: root.detail.state === "unavailable" ? qsTr("GameBanana reports this mod as private or removed.")
                        : root.detail.state === "partial" ? qsTr("Only a short summary is saved. Refresh for full details.") : qsTr("No details saved yet. Refresh to load them.")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.secondary
                }
                ModPreview {
                    id: preview
                    Layout.fillWidth: true
                    Layout.preferredHeight: Math.min(Math.round(width * 9 / 16), 360, root.height * 0.36)
                    radius: Theme.radius
                    visible: !!root.summary.identity
                    source: root.sources[root.galleryIndex] || ""
                    contextKey: root.mediaContext
                    description: qsTr("Preview of %1").arg(root.summary.title || "")
                    placeholderTitle: root.summary.title || ""
                    hidden: root.imageHidden
                    allowReveal: true
                    active: root.mediaAvailable
                    decodeSize: Qt.size(1920, 1080)
                    gallery: true
                    color: Theme.surfaceLow
                    TapHandler { enabled: preview.ready && !root.viewerOpen; onTapped: root.openViewer(largerButton) }
                    HoverHandler { cursorShape: preview.ready ? Qt.PointingHandCursor : Qt.ArrowCursor }
                    Row {
                        anchors.left: parent.left
                        anchors.bottom: parent.bottom
                        anchors.margins: 10
                        spacing: 16
                        visible: root.sources.length > 1
                        OverlayButton { text: qsTr("Previous image"); icon.source: Theme.icon("chevron-left"); enabled: root.galleryIndex > 0; onClicked: --root.galleryIndex }
                        Rectangle {
                            height: 32; width: galleryCount.implicitWidth + 20; radius: 16; color: Theme.surface
                            Label { id: galleryCount; anchors.centerIn: parent; color: Theme.text; text: qsTr("%1 / %2").arg(root.galleryIndex + 1).arg(root.sources.length); textFormat: Text.PlainText }
                        }
                        OverlayButton { text: qsTr("Next image"); icon.source: Theme.icon("chevron-right"); enabled: root.galleryIndex + 1 < root.sources.length; onClicked: ++root.galleryIndex }
                    }
                    OverlayButton {
                        id: largerButton
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        anchors.margins: 10
                        text: qsTr("View larger")
                        icon.source: Theme.icon("zoom-in")
                        visible: root.sources.length > 0
                        enabled: root.mediaAvailable && preview.ready
                        onClicked: root.openViewer(largerButton)
                    }
                }
                // Title and tags
                ColumnLayout {
                    Layout.fillWidth: true
                    visible: !!root.summary.identity
                    spacing: Theme.spacing8
                    Label {
                        Layout.fillWidth: true
                        text: root.summary.title || qsTr("Mod details")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        font.family: Theme.displayFamily
                        font.pointSize: Theme.heading
                        font.weight: Font.DemiBold
                    }
                    Flow {
                        Layout.fillWidth: true
                        spacing: 6
                        Repeater {
                            model: [Theme.meaningful(root.summary.category), Theme.meaningful(root.summary.hero), (root.summary.submission || {}).submissionType === "sound" ? qsTr("Sound") : ""].filter(value => !!value)
                            StatusPill { required property var modelData; text: modelData }
                        }
                        StatusPill { visible: !!root.summary.obsolete; text: qsTr("Obsolete"); tone: "warning" }
                        StatusPill { visible: !!root.summary.developmentState; text: root.summary.developmentState || ""; tone: "muted" }
                    }
                }
                Button {
                Layout.alignment: Qt.AlignLeft
                id: downloadButton
                objectName: visible ? "catalogDetailAction" : ""
                visible: !!root.summary.identity && root.detail.state !== "unavailable" && !["enable", "disable"].includes(root.libraryAction.action)
                highlighted: root.libraryAction.action === "download"
                text: root.actionText(root.libraryAction)
                enabled: !!root.libraryAction.enabled && !root.viewerOpen
                onClicked: {
                    const action = root.controller.libraryAction(root.summary.submission || {})
                    if (action.action === "download" && action.enabled) root.downloadRequested(root.summary.submission)
                    else root.controller.runLibraryAction(root.summary.submission || {})
                }
            }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: installedRow.implicitHeight + 16
                    visible: ["enable", "disable"].includes(root.libraryAction.action)
                    color: Theme.surface
                    radius: Theme.radiusCard
                    RowLayout {
                        id: installedRow
                        anchors.fill: parent
                        anchors.margins: 8
                        Label {
                            Layout.fillWidth: true; Layout.minimumWidth: 0
                            text: (root.libraryAction.action === "disable" ? qsTr("On in %1") : qsTr("Off in %1")).arg(root.libraryAction.profileName || "")
                            textFormat: Text.PlainText; elide: Text.ElideRight
                            Accessible.name: text
                            ToolTip.visible: stateHover.hovered; ToolTip.text: text
                            HoverHandler { id: stateHover }
                        }
                        Switch {
                            id: installedSwitch
                            objectName: visible ? "catalogDetailAction" : ""
                            checked: root.libraryAction.action === "disable"
                            enabled: !!root.libraryAction.enabled && !root.viewerOpen
                            Accessible.name: (checked ? qsTr("On in %1") : qsTr("Off in %1")).arg(root.libraryAction.profileName || "")
                            onToggled: { if (root.libraryAction.enabled && !root.viewerOpen && checked !== (root.libraryAction.action === "disable")) root.controller.runLibraryAction(root.summary.submission || {}); checked = Qt.binding(() => root.libraryAction.action === "disable") }
                        }
                    }
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Theme.spacing16
                    Repeater {
                        model: [root.summary.author || "",
                            root.summary.downloads !== undefined && root.summary.downloads !== null ? qsTr("%1 downloads").arg(root.controller.displayCount(root.summary.downloads)) : "",
                            root.summary.likes !== undefined && root.summary.likes !== null ? qsTr("%1 likes").arg(root.controller.displayCount(root.summary.likes)) : "",
                            root.summary.updatedAt ? qsTr("Updated %1").arg(root.relativeDate(root.summary.updatedAt)) : "",
                            root.summary.completionPercentage !== null && root.summary.completionPercentage !== undefined ? qsTr("%1% complete").arg(root.summary.completionPercentage) : ""].filter(Boolean)
                        Label {
                            required property var modelData
                            width: Math.min(implicitWidth, parent.width)
                            text: modelData; textFormat: Text.PlainText; wrapMode: Text.Wrap
                            color: Theme.secondary
                            font.pointSize: Theme.small; font.features: { "tnum": 1 }
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: root.summary.masked || (root.detail.contentWarnings || []).length > 0
                    text: !root.summary.visibilityKnown ? qsTr("Content rating unknown; previews are hidden.") : (root.detail.contentWarnings || []).map(warning => warning.label).join("\n") || qsTr("Mature content; previews are hidden.")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.warning
                }
                // Actions
                Flow {
                    Layout.fillWidth: true
                    visible: !!root.summary.identity
                    spacing: Theme.spacing8
                    Button {
                        id: authorButton
                        visible: !!root.summary.authorId
                        flat: true; text: qsTr("More by author")
                        onActiveFocusChanged: if (activeFocus) root.rememberFocus("author")
                        onClicked: root.controller.author(root.summary.authorId)
                    }
                    Button {
                        id: favoriteButton
                        onActiveFocusChanged: if (activeFocus) root.rememberFocus("favorite")
                        visible: root.summary.favoriteKnown === true
                        enabled: !root.controller.favoriteBusy
                        icon.source: Theme.icon(root.summary.favorite ? "heart-filled" : "heart")
                        icon.color: root.summary.favorite ? Theme.accentInk : Theme.text
                        text: root.summary.favorite ? qsTr("Favorited") : qsTr("Favorite")
                        onClicked: root.controller.setFavorite(root.summary.submission, !root.summary.favorite)
                    }
                    Button {
                        id: providerButton
                        onActiveFocusChanged: if (activeFocus) root.rememberFocus("provider")
                        visible: !!root.summary.profileUrl
                        flat: true
                        text: qsTr("View on GameBanana")
                        icon.source: Theme.icon("external")
                        onClicked: root.controller.openLink(root.summary.profileUrl)
                    }
                }
                // Tabs
                RowLayout {
                    Layout.fillWidth: true
                    visible: !!root.summary.identity && root.detail.state !== "unavailable"
                    spacing: Theme.spacing4
                    TabButton { checkable: false; checked: root.tab === "overview"; text: qsTr("Overview"); onClicked: root.openTab("overview") }
                    TabButton { id: changelogButton; checkable: false; checked: root.tab === "changelog"; onActiveFocusChanged: if (activeFocus) root.rememberFocus("changelog"); text: qsTr("Changelog"); onClicked: root.openTab("changelog") }
                    TabButton { id: commentsButton; checkable: false; checked: root.tab === "comments"; onActiveFocusChanged: if (activeFocus) root.rememberFocus("comments"); text: qsTr("Comments"); onClicked: root.openTab("comments") }
                    TabButton { id: filesButton; checkable: false; checked: root.tab === "files"; onActiveFocusChanged: if (activeFocus) root.rememberFocus("files"); text: qsTr("Files"); onClicked: root.openTab("files") }
                    Item { Layout.fillWidth: true }
                }
                Rectangle { Layout.fillWidth: true; Layout.topMargin: -Theme.spacing12; height: 1; color: Theme.rule; visible: !!root.summary.identity }
                // Overview
                ColumnLayout {
                    Layout.fillWidth: true
                    visible: root.tab === "overview"
                    spacing: Theme.spacing12
                    RichDescription {
                        Layout.fillWidth: true
                        Layout.maximumWidth: 860
                        blocks: root.detail.richDescription || []
                        visible: blocks.length > 0
                        controller: root.controller
                        contextKey: root.mediaContext
                        hidden: root.imageHidden
                    }
                    TextEdit {
                        id: descriptionText
                        Layout.fillWidth: true
                        Layout.maximumWidth: 860
                        visible: !(root.detail.richDescription || []).length && !!text
                        text: root.detail.description || root.summary.description || ""
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        font.pointSize: Theme.body
                        readOnly: true
                        selectByMouse: true
                        color: Theme.text
                        Accessible.name: qsTr("Plain submission description")
                    }
                    Label { Layout.fillWidth: true; visible: !!(root.detail.metadata || {}).mapName; text: qsTr("Map: %1").arg((root.detail.metadata || {}).mapName || ""); textFormat: Text.PlainText; wrapMode: Text.Wrap }
                    Label { visible: (root.detail.dependencies || []).length > 0; text: qsTr("Requires"); font.weight: Font.DemiBold }
                    CatalogLinks { Layout.fillWidth: true; controller: root.controller; links: root.detail.dependencies || [] }
                    Label { visible: root.descriptionLinks.length > 0; text: qsTr("Links"); font.weight: Font.DemiBold }
                    CatalogLinks { Layout.fillWidth: true; controller: root.controller; links: root.descriptionLinks }
                    Label { visible: root.supportLinks.length > 0; text: qsTr("Support the author"); font.weight: Font.DemiBold }
                    CatalogLinks { Layout.fillWidth: true; controller: root.controller; links: root.supportLinks }
                    Label { visible: !!root.detail.linksTruncated; text: qsTr("Some links could not be displayed."); textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true; color: Theme.secondary }
                }
                Loader {
                    Layout.fillWidth: true
                    active: root.tab !== "overview" && root.controller.resourceKind !== "" && root.detail.state !== "unavailable"
                    sourceComponent: CatalogResource { controller: root.controller }
                }
            }
        }
    }
}
