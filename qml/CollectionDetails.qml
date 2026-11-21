pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

FocusScope {
    id: root
    required property CatalogController controller
    property bool installAvailable: false
    signal downloadRequested(var submission)
    readonly property var detail: controller.detail
    readonly property var header: detail.header || {}
    property bool sourceDetailsExpanded: false
    readonly property var images: header.images || []
    readonly property var headerSources: images.length ? images : header.coverUrl ? [header.coverUrl] : []
    readonly property bool headerHidden: header.isNsfw === undefined || !!header.isNsfw || !header.contentRatingKnown
    readonly property string headerContext: JSON.stringify(["collection", header.collectionId || "", controller.viewGeneration])
    readonly property bool routeReady: visible && controller.active && controller.route === "collection"
                                       && controller.contentReady && detail.state === "ready" && !!header.collectionId
    property int galleryIndex: 0
    property var generation: 0
    property bool restoring: true
    property bool memberModelCoherent: true
    property int memberRevision: 0
    property int tokenCounter: 0
    property bool viewerNavigating: false
    property int viewerNavigationRevision: 0
    readonly property bool viewerOpen: viewer.opened
    FontMetrics { id: statusMetrics; font.pointSize: Theme.body }

    function issueToken() { return "collection-media:" + controller.viewGeneration + ":" + (++tokenCounter) }
    function memberSnapshot(identity, revision) {
        if (!memberModelCoherent || !identity) return null
        const row = controller.model.item(identity)
        if (!row || row.identity !== identity || row.state !== "ready") return null
        const metadata = row.metadata || {}
        if (metadata.identity !== identity) return null
        const sources = []
        const images = metadata.images || []
        for (let i = 0; i < images.length; ++i) sources.push(String(images[i]))
        return {identity: identity, source: String(metadata.thumbnail || ""), sources: sources,
                hidden: metadata.masked === undefined || !!metadata.masked, title: String(metadata.title || "")}
    }
    function memberInRange(item) {
        if (!item || !routeReady || !memberModelCoherent) return false
        const point = item.mapToItem(members.contentItem, 0, 0)
        return point.y + item.height > members.contentY - 120 && point.y < members.contentY + members.height + 120
    }
    function viewerValid() {
        const state = viewer.snapshot
        if (!state || !routeReady || controller.viewGeneration !== state.generation || String(header.collectionId) !== state.collection
                || controller.selectedIdentity !== state.selection || members.currentIndex !== state.selectionIndex) return false
        if (state.kind === "header") {
            if (headerHidden !== state.hidden || headerContext !== state.token || String(header.name || "") !== state.title
                    || !viewer.sameSources(headerSources, state.sources) || !headerPreview.active
                    || headerPreview.hidden !== state.hidden || headerPreview.contextKey !== state.token) return false
            return viewerNavigating || (galleryIndex === viewer.currentIndex
                    && headerPreview.source.toString() === String(state.sources[viewer.currentIndex] || ""))
        }
        const owner = viewer.owner
        if (state.kind !== "member" || !memberModelCoherent || memberRevision !== state.revision || !owner || !owner.mediaActive
                || owner.identity !== state.identity || owner.mediaToken !== state.token) return false
        const current = memberSnapshot(state.identity, memberRevision)
        if (!current || current.identity !== state.identity || current.source !== state.source || current.hidden !== state.hidden
                || current.title !== state.title || !viewer.sameSources(viewer.slots(current.sources, current.source, -1).sources, state.sources)) return false
        return owner.preview.source.toString() === state.source && owner.preview.contextKey === state.token
                && owner.preview.hidden === state.hidden && owner.preview.active
    }
    function openHeaderViewer(invoker) {
        if (viewer.requested || !routeReady) return
        const source = headerPreview.source.toString()
        const opening = viewer.slots(headerSources, source, galleryIndex)
        if (!viewer.sameSources(opening.sources, headerSources)) return
        viewerNavigating = false
        viewer.open({kind: "header", identity: String(header.collectionId), token: headerContext,
                     collection: String(header.collectionId), selection: controller.selectedIdentity,
                     generation: controller.viewGeneration, selectionIndex: members.currentIndex,
                     source: source, title: String(header.name || ""), hidden: headerHidden,
                     sources: opening.sources, startIndex: opening.index}, headerPreview, invoker)
    }
    function openMemberViewer(owner, invoker) {
        if (viewer.requested || !routeReady || !owner || !owner.mediaActive) return
        const current = memberSnapshot(owner.identity, memberRevision)
        if (!current || current.identity !== owner.identity || current.source !== owner.preview.source.toString()
                || current.hidden !== owner.preview.hidden || owner.preview.contextKey !== owner.mediaToken) return
        const opening = viewer.slots(current.sources, current.source, -1)
        viewer.open({kind: "member", identity: current.identity, token: owner.mediaToken,
                     collection: String(header.collectionId), selection: controller.selectedIdentity,
                     generation: controller.viewGeneration, selectionIndex: members.currentIndex, revision: memberRevision,
                     source: current.source, title: current.title, hidden: current.hidden,
                     sources: opening.sources, startIndex: opening.index}, owner.preview, invoker, owner)
    }
    function closeViewer() { viewer.close() }
    onGalleryIndexChanged: if (headerPreview) headerPreview.revealed = false
    function date(value) { return value === null || value === undefined ? qsTr("Unknown age") : new Date(Number(value) * 1000).toLocaleString(Qt.locale()) }
    function restore() {
        if (!root.controller.contentReady) return
        root.restoring = true
        members.currentIndex = root.controller.model.indexOf(root.controller.selectedIdentity)
        members.contentY = Math.max(members.originY, root.controller.scrollPosition)
        if (!root.viewerOpen) {
            if (root.controller.focusKey === "results") members.forceActiveFocus()
            else back.forceActiveFocus()
        }
        Qt.callLater(() => root.restoring = false)
    }
    Component.onCompleted: { generation = controller.viewGeneration; Qt.callLater(restore) }
    Connections {
        target: root.controller
        function onChanged() {
            if (root.generation !== root.controller.viewGeneration) {
                root.generation = root.controller.viewGeneration
                root.galleryIndex = 0
                root.restoring = true
                Qt.callLater(root.restore)
            } else {
                if (root.galleryIndex >= root.headerSources.length) root.galleryIndex = 0
                if (root.restoring && root.controller.contentReady) Qt.callLater(root.restore)
            }
        }
    }
    Connections {
        target: root.controller.model
        function onCountChanged() { Qt.callLater(root.restore) }
        function onModelAboutToBeReset() { root.memberModelCoherent = false; ++root.memberRevision }
        function onModelReset() { ++root.memberRevision; root.memberModelCoherent = true }
        function onRowsAboutToBeRemoved() { root.memberModelCoherent = false; ++root.memberRevision }
        function onRowsRemoved() { ++root.memberRevision; root.memberModelCoherent = true }
        function onRowsAboutToBeInserted() { root.memberModelCoherent = false; ++root.memberRevision }
        function onRowsInserted() { ++root.memberRevision; root.memberModelCoherent = true }
        function onLayoutAboutToBeChanged() { root.memberModelCoherent = false; ++root.memberRevision }
        function onLayoutChanged() { ++root.memberRevision; root.memberModelCoherent = true }
        function onDataChanged() { ++root.memberRevision }
    }
    ViewerSession {
        id: viewer
        valid: root.viewerValid()
        title: snapshot ? snapshot.title || qsTr("Mod preview") : ""
        onFocusRequested: (state, owner, target, index) => {
            if (!root.visible || !root.controller.active || root.controller.route !== "collection") return
            if (root.routeReady && target && target.visible && target.enabled && root.controller.viewGeneration === state.generation
                    && String(root.header.collectionId) === state.collection && root.controller.selectedIdentity === state.selection
                    && members.currentIndex === state.selectionIndex
                    && ((state.kind === "header" && root.headerContext === state.token && root.headerHidden === state.hidden
                         && root.galleryIndex === index && viewer.sameSources(root.headerSources, state.sources))
                        || (state.kind === "member" && owner && owner.identity === state.identity && owner.mediaToken === state.token && owner.mediaActive)))
                target.forceActiveFocus()
            else if (root.memberModelCoherent && members.visible && members.enabled) members.forceActiveFocus()
            else if (back.visible && back.enabled) back.forceActiveFocus()
            else root.forceActiveFocus()
        }
        onIndexSelected: index => {
            if (viewer.snapshot.kind !== "header") return
            root.viewerNavigating = true
            root.galleryIndex = index
            const revision = ++root.viewerNavigationRevision
            Qt.callLater(() => {
                if (root && revision === root.viewerNavigationRevision) root.viewerNavigating = false
            })
        }
        onImageRevealed: (index, source, contextKey) => {
            const state = viewer.snapshot
            if (state.kind === "header" && index === root.galleryIndex
                    && headerPreview.contextKey === contextKey && headerPreview.source.toString() === source
                    && headerPreview.hidden === state.hidden && headerPreview.active)
                headerPreview.revealed = true
            else if (state.kind === "member" && index === state.startIndex && source === state.source) {
                const owner = viewer.owner
                if (owner && owner.mediaActive && owner.mediaToken === contextKey && owner.preview.contextKey === contextKey
                        && owner.preview.source.toString() === source && owner.preview.hidden === state.hidden)
                    owner.preview.revealed = true
            }
        }
    }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Button { id: back; flat: true; icon.source: Theme.icon("arrow-left"); text: qsTr("Back"); enabled: root.controller.canBack; onClicked: root.controller.back() }
            Label { Layout.fillWidth: true; text: root.header.name || qsTr("Collection"); textFormat: Text.PlainText; font.pointSize: Theme.heading; wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight }
            Button { text: qsTr("GameBanana"); icon.source: Theme.icon("external"); visible: !!root.header.profileUrl; onClicked: root.controller.openLink(root.header.profileUrl) }
        }
        CatalogStatus { Layout.fillWidth: true; controller: root.controller }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(collectionMetadata.implicitHeight, 260)
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                id: collectionMetadata
                width: parent.width
                RowLayout {
                    Layout.fillWidth: true
                    ModPreview {
                        id: headerPreview
                        Layout.preferredWidth: 180
                        Layout.preferredHeight: 110
                        source: root.headerSources.length ? root.headerSources[Math.min(root.galleryIndex, root.headerSources.length - 1)] : ""
                        contextKey: root.headerContext
                        description: qsTr("Cover of %1").arg(root.header.name || "")
                        decodeSize: Qt.size(360, 220)
                        hidden: root.headerHidden
                        allowReveal: true
                        active: root.routeReady
                        MouseArea {
                            x: (headerPreview.width - headerPreview.imagePaintedSize.width) / 2
                            y: (headerPreview.height - headerPreview.imagePaintedSize.height) / 2
                            width: headerPreview.imagePaintedSize.width
                            height: Math.max(0, Math.min(headerPreview.imagePaintedSize.height,
                                                        headerPreview.height - (headerPreview.remote ? statusMetrics.height + 12 : 1) - y))
                            enabled: root.routeReady && headerPreview.ready && !root.viewerOpen
                            acceptedButtons: Qt.LeftButton
                            cursorShape: Qt.PointingHandCursor
                            onPressed: mouse => mouse.accepted = true
                            onClicked: mouse => { mouse.accepted = true; root.openHeaderViewer(headerLarger) }
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Label { Layout.fillWidth: true; text: root.header.author || ""; textFormat: Text.PlainText; wrapMode: Text.Wrap }
                        Button { visible: !!root.header.authorRemoteId; text: qsTr("Author’s submissions"); onClicked: root.controller.author(root.header.authorRemoteId) }
                        Label { Layout.fillWidth: true; text: qsTr("Members: %1 listed · %2 expected").arg(root.detail.fetchedMembershipCount == null ? qsTr("Unknown") : root.detail.fetchedMembershipCount).arg(root.detail.expectedCount == null ? qsTr("Unknown") : root.detail.expectedCount); textFormat: Text.PlainText; wrapMode: Text.Wrap }
                    }
                }
                Flow {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    spacing: 6
                    visible: root.headerSources.length > 0
                    Button { text: qsTr("Previous image"); visible: root.headerSources.length > 1; enabled: root.galleryIndex > 0; onClicked: --root.galleryIndex }
                    Label { padding: 8; visible: root.headerSources.length > 1; text: qsTr("%1 of %2").arg(root.galleryIndex + 1).arg(root.headerSources.length) }
                    Button { text: qsTr("Next image"); visible: root.headerSources.length > 1; enabled: root.galleryIndex + 1 < root.headerSources.length; onClicked: ++root.galleryIndex }
                    Button {
                        id: headerLarger
                        text: qsTr("View larger")
                        Accessible.name: qsTr("View larger image of %1").arg(root.header.name || qsTr("Collection"))
                        enabled: root.routeReady && headerPreview.ready
                        onClicked: root.openHeaderViewer(headerLarger)
                    }
                }
                Label { Layout.fillWidth: true; Layout.maximumWidth: 820; visible: root.controller.contentReady; text: root.header.description || (root.detail.state === "missing" ? qsTr("Collection header is not cached. Refresh the catalog before fetching this collection.") : qsTr("No cached description.")); textFormat: Text.PlainText; wrapMode: Text.Wrap }
                Label {
                    Layout.fillWidth: true
                    visible: !!root.detail.descriptionStale || !!root.detail.membershipStale
                    text: qsTr("Some collection information may be out of date.")
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: Number(root.detail.unavailableOrMissingMemberCount || 0) > 0
                    text: qsTr("Some members are unavailable or missing: %1.").arg(root.detail.unavailableOrMissingMemberCount)
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                Label { Layout.fillWidth: true; text: qsTr("Collection members don't show file or dependency details."); wrapMode: Text.Wrap; color: Theme.secondary; font.pointSize: Theme.small }
                Button {
                    text: root.sourceDetailsExpanded ? qsTr("Hide collection source details") : qsTr("Collection source details")
                    onClicked: root.sourceDetailsExpanded = !root.sourceDetailsExpanded
                }
                Label {
                    Layout.fillWidth: true
                    visible: root.sourceDetailsExpanded
                    text: qsTr("Header (%1): %2\nDescription (%3): %4%5\nMembership (%6): %7%8\nUnavailable or missing members: %9. Member metadata does not establish files, dependencies or installability.")
                        .arg(root.detail.headerSource || qsTr("Unknown source")).arg(root.date(root.header.headerFetchedAt))
                        .arg(root.detail.descriptionSource || qsTr("Unknown source")).arg(root.date(root.detail.descriptionFetchedAt)).arg(root.detail.descriptionStale ? qsTr(" • stale") : "")
                        .arg(root.detail.membershipSource || qsTr("Unknown source")).arg(root.date(root.detail.lastMemberSyncAt)).arg(root.detail.membershipStale ? qsTr(" • stale") : "")
                        .arg(root.detail.unavailableOrMissingMemberCount || qsTr("Unknown"))
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.pointSize: Theme.small
                    color: Theme.secondary
                }
            }
        }
        ListView {
            id: members
            Layout.fillWidth: true
            Layout.fillHeight: true
            model: root.controller.model
            activeFocusOnTab: true
            clip: true
            reuseItems: true
            ScrollBar.vertical: ScrollBar {}
            onContentYChanged: if (!root.restoring) root.controller.remember(root.controller.selectedIdentity, contentY, activeFocus ? "results" : root.controller.focusKey, root.generation)
            onActiveFocusChanged: if (activeFocus && !root.restoring) root.controller.remember(root.controller.selectedIdentity, contentY, "results", root.generation)
            Keys.onReturnPressed: if (currentItem) root.controller.openItem(currentItem.identity)
            delegate: ItemDelegate {
                id: member
                required property string identity
                required property string title
                required property string author
                required property var model
                required property string reason
                required property string thumbnail
                required property bool contentMasked
                required property var submission
                required property bool favoriteKnown
                required property bool favorite
                required property int index
                property bool pooled: false
                property bool armed: false
                property bool initialized: false
                property int armRevision: 0
                property string mediaToken: ""
                property alias preview: memberPreview
                readonly property var ownerSnapshot: root.memberSnapshot(identity, root.memberRevision)
                readonly property bool inRange: {
                    const position = x + y + width + height + members.contentY + members.height + root.memberRevision
                    return root.memberInRange(member)
                }
                readonly property bool coherentOwner: root.routeReady && root.memberModelCoherent && visible && !pooled && inRange
                                                       && model.state === "ready" && ownerSnapshot && ownerSnapshot.identity === identity
                                                       && ownerSnapshot.source === thumbnail && ownerSnapshot.hidden === contentMasked
                                                       && ownerSnapshot.title === title
                readonly property bool mediaActive: armed && coherentOwner
                function resetMedia() {
                    if (!initialized) return
                    const serial = ++armRevision
                    armed = false
                    memberPreview.revealed = false
                    mediaToken = root.issueToken()
                    if (!coherentOwner) return
                    const snapshot = ownerSnapshot
                    const revision = root.memberRevision
                    const token = mediaToken
                    Qt.callLater(() => {
                        if (!member || serial !== member.armRevision || revision !== root.memberRevision
                                || token !== member.mediaToken || !member.coherentOwner) return
                        const current = root.memberSnapshot(member.identity, revision)
                        if (!current || current.identity !== snapshot.identity || current.source !== snapshot.source
                                || current.hidden !== snapshot.hidden || current.title !== snapshot.title
                                || !viewer.sameSources(current.sources, snapshot.sources)) return
                        member.armed = true
                    })
                }
                onIdentityChanged: resetMedia()
                onOwnerSnapshotChanged: resetMedia()
                onCoherentOwnerChanged: resetMedia()
                onPooledChanged: resetMedia()
                Component.onCompleted: { initialized = true; resetMedia() }
                ListView.onPooled: pooled = true
                ListView.onReused: { pooled = false; resetMedia() }
                width: members.width
                highlighted: members.currentIndex === index || root.controller.selectedIdentity === identity
                enabled: model.state === "ready" || model.state === "missing"
                Accessible.name: qsTr("%1, %2").arg(title).arg(model.state)
                onClicked: { root.controller.remember(identity, members.contentY, "results", root.generation); root.controller.openItem(identity) }
                contentItem: RowLayout {
                    ColumnLayout {
                        ModPreview {
                            id: memberPreview
                            Layout.preferredWidth: 90
                            Layout.preferredHeight: 56
                            source: member.ownerSnapshot ? member.ownerSnapshot.source : ""
                            hidden: !member.ownerSnapshot || member.ownerSnapshot.hidden
                            contextKey: member.mediaToken
                            description: qsTr("Preview of %1").arg(member.title)
                            active: member.mediaActive
                            compact: true
                            decodeSize: Qt.size(180, 112)
                            MouseArea {
                                x: (memberPreview.width - memberPreview.imagePaintedSize.width) / 2
                                y: (memberPreview.height - memberPreview.imagePaintedSize.height) / 2
                                width: memberPreview.imagePaintedSize.width
                                height: memberPreview.imagePaintedSize.height
                                enabled: member.mediaActive && memberPreview.ready && !root.viewerOpen
                                acceptedButtons: Qt.LeftButton
                                propagateComposedEvents: false
                                cursorShape: Qt.PointingHandCursor
                                onPressed: mouse => mouse.accepted = true
                                onClicked: mouse => { mouse.accepted = true; root.openMemberViewer(member, memberLarger) }
                            }
                        }
                        Button {
                            visible: member.coherentOwner && memberPreview.masked && !!member.ownerSnapshot.source
                            enabled: member.mediaActive
                            text: qsTr("Show preview")
                            Accessible.name: qsTr("Show preview of %1").arg(member.title)
                            onClicked: if (member.mediaActive && memberPreview.masked) memberPreview.revealed = true
                            Keys.onReturnPressed: event => { event.accepted = true; if (enabled && member.mediaActive && memberPreview.masked) memberPreview.revealed = true }
                            Keys.onEnterPressed: event => { event.accepted = true; if (enabled && member.mediaActive && memberPreview.masked) memberPreview.revealed = true }
                        }
                        Button {
                            id: memberLarger
                            visible: member.model.state === "ready"
                            enabled: member.mediaActive && memberPreview.ready && !memberPreview.masked
                            text: qsTr("View larger")
                            Accessible.name: qsTr("View larger image of %1").arg(member.title)
                            onClicked: root.openMemberViewer(member, memberLarger)
                            Keys.onReturnPressed: event => { event.accepted = true; if (enabled) root.openMemberViewer(member, memberLarger) }
                            Keys.onEnterPressed: event => { event.accepted = true; if (enabled) root.openMemberViewer(member, memberLarger) }
                        }
                    }
                    Label { Layout.fillWidth: true; text: qsTr("%1\n%2%3").arg(member.title).arg(member.model.state === "ready" ? member.author : member.model.state === "unidentified" ? qsTr("Unsupported or redacted member") : member.model.state === "unavailable" ? qsTr("Private or unavailable") : qsTr("Summary missing from catalog")).arg(member.reason ? " (" + member.reason + ")" : ""); textFormat: Text.PlainText; wrapMode: Text.Wrap }
                    Button {
                        visible: member.model.state === "ready"
                        highlighted: true
                        enabled: root.installAvailable && !!member.submission && !root.viewerOpen
                        text: qsTr("Install")
                        icon.source: Theme.icon("download")
                        Accessible.name: qsTr("Install %1").arg(member.title)
                        onClicked: root.downloadRequested(member.submission)
                    }
                    Button {
                        visible: member.model.state === "ready"
                        enabled: member.favoriteKnown && !root.controller.favoriteBusy
                        flat: true
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon(member.favorite ? "heart-filled" : "heart")
                        icon.color: member.favorite ? Theme.accentInk : Theme.secondary
                        text: member.favorite ? qsTr("Unfavorite") : qsTr("Favorite")
                        onClicked: root.controller.setFavorite(member.submission, !member.favorite)
                        Keys.onReturnPressed: event => { event.accepted = true; if (enabled) root.controller.setFavorite(member.submission, !member.favorite) }
                        Keys.onEnterPressed: event => { event.accepted = true; if (enabled) root.controller.setFavorite(member.submission, !member.favorite) }
                    }
                }
            }
            Label { anchors.centerIn: parent; width: parent.width; horizontalAlignment: Text.AlignHCenter; visible: members.count === 0 && !root.controller.busy && root.controller.contentReady && root.controller.error === ""; text: root.detail.state === "missing" ? qsTr("Collection details are unavailable. Refresh to try again.") : qsTr("No members on this cached page."); textFormat: Text.PlainText; wrapMode: Text.Wrap }
        }
        RowLayout {
            Layout.fillWidth: true
            Button { text: qsTr("Previous page"); enabled: root.controller.page > 0; onClicked: root.controller.nextPage(-1) }
            Label { Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter; text: qsTr("Member page %1").arg(Number(root.controller.page + 1).toLocaleString()) }
            Button { text: qsTr("Next page"); enabled: root.controller.hasNext; onClicked: root.controller.nextPage(1) }
        }
    }
}
