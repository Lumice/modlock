pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle

FocusScope {
    id: root
    required property var stateBank
    required property var model
    required property Component delegateComponent
    required property string scope
    required property string contextKey
    required property string domain
    required property string selectedIdentity
    required property string focusKey
    required property var indexForIdentity
    required property var identityAt
    required property var snapshotFor
    property string presentationMode: "list"
    property bool ready: true
    property bool viewerAllowed: true
    property var ownerEpoch: 0
    property real initialScroll: 0
    property real minimumCellWidth: 230
    readonly property real cellWidth: presentationMode === "grid" ? width / Math.max(1, Math.floor(width / minimumCellWidth)) : width
    readonly property real listHeight: Math.max(64, Math.ceil(bodyMetrics.height * 3 + 16))
    // Poster cards (see MediaCard): square art fills a Library card; Browse adds a footer
    // strip for stats and actions. Cards are 8 px smaller than their cell. Library stays
    // square to retain the existing compact grid density. The 4:5 experiments
    // changed flicker/scroll results; they do not establish a general geometry limit.
    readonly property int posterFooterHeight: 52
    readonly property real gridHeight: 8 + Math.max(Math.ceil(bodyMetrics.height * 8),
        Math.ceil(Math.max(0, cellWidth - 8)) + (scope === "library" ? 0 : posterFooterHeight))
    FontMetrics { id: bodyMetrics; font.pointSize: Theme.body }
    readonly property var view: viewLoader.item
    readonly property int count: view ? view.count : 0
    property bool initialized: false
    property bool modelCoherent: true
    property int modelRevision: 0
    // Interaction ownership survives role updates; structure/owner changes do not.
    property int changeRevision: 0
    property int restoreRevision: 0
    property bool restoring: true
    property string activeContext: ""
    property var pendingBookmark: null
    property bool pendingFocus: false
    property bool captureQueued: false
    property bool userInterrupted: false
    property int interruptionSerial: 0
    property Item interruptedView: null
    property string interruptedContext: ""
    property int interruptedIndex: -1
    property int interruptedRevision: -1
    property int pendingNavigationCaptures: 0
    property bool enterConsumed: false
    property bool enterArmed: false
    property int enterGeneration: 0
    readonly property bool viewerOpen: viewer.opened
    signal selectionRequested(string identity)
    signal detailsRequested(string identity)
    signal positionChanged(real position, string focus)
    signal searchFocusRequested()

    function stopEnterWindow() {
        enterWindowTimer.stop()
        enterArmed = false
    }
    function evaluateEnter() {
        if (enterConsumed || !visible || !ready || !modelCoherent || count <= 0) return
        enterConsumed = true
        const alreadyVisited = stateBank && ("hasEnteredPage" in stateBank) && stateBank.hasEnteredPage(scope)
        if (stateBank && ("markPageEntered" in stateBank)) stateBank.markPageEntered(scope)
        if (alreadyVisited || Tokens.reduceMotion || Tokens.normal <= 0) return
        ++enterGeneration
        enterArmed = true
        enterWindowTimer.restart()
    }
    Timer {
        id: enterWindowTimer
        interval: 450
        onTriggered: root.enterArmed = false
    }
    Connections {
        target: Tokens
        function onReduceMotionChanged() { if (Tokens.reduceMotion) root.stopEnterWindow() }
    }
    function issueToken() { return stateBank.issueToken() }
    function viewerValid() {
        const revision = modelRevision
        const state = viewer.snapshot
        const owner = viewer.owner
        if (!state || !visible || !ready || !viewerAllowed || !modelCoherent || !owner || !owner.mediaActive
                || owner.anchorIdentity !== state.identity || owner.mediaToken !== state.token
                || contextKey !== state.context || selectedIdentity !== state.selection || ownerEpoch !== state.epoch) return false
        const current = snapshotFor(state.identity, revision)
        if (!current || current.identity !== state.identity || String(current.source || "") !== state.source
                || !!current.hidden !== state.hidden || String(current.title || "") !== state.title
                || !viewer.sameSources(viewer.slots(current.sources || [], String(current.source || ""), -1).sources, state.sources)) return false
        return owner.preview.source.toString() === state.source && owner.preview.contextKey === state.token
                && owner.preview.hidden === state.hidden && owner.preview.active
    }
    function openViewer(owner, invoker) {
        userInteracted()
        if (!visible || !ready || !modelCoherent || viewer.requested || !viewerAllowed || !owner || !owner.mediaActive) return
        const current = snapshotFor(owner.anchorIdentity, modelRevision)
        if (!current || current.identity !== owner.anchorIdentity || String(current.source || "") !== owner.preview.source.toString()
                || !!current.hidden !== owner.preview.hidden || owner.preview.contextKey !== owner.mediaToken) return
        const opening = viewer.slots(current.sources || [], String(current.source || ""), -1)
        viewer.open({identity: current.identity, token: owner.mediaToken, context: contextKey,
                     selection: selectedIdentity, epoch: ownerEpoch, source: String(current.source),
                     title: String(current.title || ""), hidden: !!current.hidden,
                     sources: opening.sources, startIndex: opening.index}, owner.preview, invoker, owner)
    }
    function closeViewer() { viewer.close() }
    ViewerSession {
        id: viewer
        valid: root.viewerValid()
        onFocusRequested: (state, owner, target, index) => {
            if (!root.visible || !root.ready || !root.modelCoherent) return
            if (owner && target && owner.anchorIdentity === state.identity && owner.mediaToken === state.token
                    && root.contextKey === state.context && root.selectedIdentity === state.selection && root.ownerEpoch === state.epoch
                    && owner.mediaActive && target.visible && target.enabled) target.forceActiveFocus()
            else if (root.view && root.view.visible && root.view.enabled) root.view.forceActiveFocus()
            else root.searchFocusRequested()
        }
        onImageRevealed: (index, source, contextKey) => {
            const state = viewer.snapshot
            const owner = viewer.owner
            if (owner && index === state.startIndex && source === state.source
                    && owner.mediaToken === contextKey && owner.preview.active && owner.preview.hidden === state.hidden
                    && owner.preview.source.toString() === source) owner.preview.revealed = true
        }
    }

    function inRange(item) {
        const current = view
        // Loading and model notifications do not move an unchanged card out of the viewport.
        // MediaCard separately guards arming changed media until the model is ready.
        if (!current || !item || !visible) return false
        const dependencies = item.x + item.y + current.contentY + current.width + current.height + modelRevision
        const point = item.mapToItem(current.contentItem, 0, 0)
        const overscan = 120
        return point.y + item.height > current.contentY - overscan && point.y < current.contentY + current.height + overscan
                && point.x + item.width > 0 && point.x < current.width
    }
    function capture() {
        const current = view
        if (!initialized || restoring || !modelCoherent || !ready || !current || !activeContext || current.height <= 0) return
        const top = current.contentY
        let best = null
        let bestY = Infinity
        let bestX = Infinity
        let next = null
        let nextY = Infinity
        const children = current.contentItem.children
        for (let i = 0; i < children.length; ++i) {
            const item = children[i]
            if (!("anchorIdentity" in item) || !item.anchorIdentity || item.pooled || !item.visible || item.height <= 0) continue
            const point = item.mapToItem(current.contentItem, 0, 0)
            if (point.y + item.height > top && point.y < top + current.height) {
                if (point.y < bestY || (point.y === bestY && point.x < bestX)) { best = item; bestY = point.y; bestX = point.x }
            } else if (point.y >= top && point.y < nextY && point.y - top <= 32) { next = item; nextY = point.y }
        }
        if (!best && next) { best = next; bestY = nextY }
        if (!best) return
        stateBank.put(scope, activeContext, {
            identity: best.anchorIdentity,
            offset: Math.max(-32, Math.min(top - bestY, Math.max(0, best.height - 1))),
            index: best.index,
            selected: selectedIdentity,
            focus: current.activeFocus ? "results" : focusKey
        })
    }
    function queueCapture() {
        if (captureQueued) return
        captureQueued = true
        Qt.callLater(() => { if (root) { root.captureQueued = false; root.capture() } })
    }
    function clearInterruption() {
        ++interruptionSerial
        userInterrupted = false
        interruptedView = null
        interruptedContext = ""
        interruptedIndex = -1
        interruptedRevision = -1
        pendingNavigationCaptures = 0
    }
    function navigationKey(key) {
        return key === Qt.Key_Up || key === Qt.Key_Down || key === Qt.Key_Left || key === Qt.Key_Right
                || key === Qt.Key_Home || key === Qt.Key_End || key === Qt.Key_PageUp || key === Qt.Key_PageDown
    }
    function userInteracted(navigating) {
        if (!userInterrupted && (restoring || !modelCoherent)) {
            ++restoreRevision
            restoring = true
            pendingBookmark = null
            pendingFocus = false
            userInterrupted = true
            interruptedView = view
            interruptedContext = activeContext
            interruptedIndex = -1
            interruptedRevision = -1
            ++interruptionSerial
            stateBank.remove(scope, activeContext)
        }
        if (!userInterrupted) return
        if (navigating && view) {
            const current = view
            const previousIndex = current.currentIndex
            const revision = changeRevision
            const context = activeContext
            const serial = interruptionSerial
            ++pendingNavigationCaptures
            Qt.callLater(() => {
                if (!root || serial !== root.interruptionSerial) return
                if (root.view === current && root.activeContext === context && root.contextKey === context
                        && root.changeRevision === revision && current.currentIndex !== previousIndex
                        && current.currentIndex >= 0 && current.currentIndex < current.count) {
                    root.interruptedIndex = current.currentIndex
                    root.interruptedRevision = revision
                }
                --root.pendingNavigationCaptures
                root.finishInterruptedInput()
            })
        } else Qt.callLater(() => { if (root) root.finishInterruptedInput() })
    }
    function finishInterruptedInput() {
        const current = view
        if (!userInterrupted || !modelCoherent || !ready || pendingNavigationCaptures > 0
                || !current || current !== interruptedView || activeContext !== interruptedContext
                || contextKey !== activeContext || current.width <= 1 || current.height <= 1) return
        const position = current.contentY
        const navigationIndex = interruptedIndex
        const navigationRevision = interruptedRevision
        const navigation = interruptedRevision === changeRevision && interruptedIndex >= 0
                           && interruptedIndex < current.count && current.currentIndex === interruptedIndex
        const selected = selectedIdentity ? indexForIdentity(selectedIdentity) : -1
        if (!navigation) current.currentIndex = selected >= 0 && selected < current.count ? selected : -1
        current.forceLayout()
        const navigationStillValid = navigation && changeRevision === navigationRevision && current.currentIndex === navigationIndex
        if (navigation && !navigationStillValid) {
            const liveSelected = selectedIdentity ? indexForIdentity(selectedIdentity) : -1
            current.currentIndex = liveSelected >= 0 && liveSelected < current.count ? liveSelected : -1
            current.forceLayout()
        }
        const upper = current.originY + Math.max(0, current.contentHeight - current.height)
        current.contentY = Math.max(current.originY, Math.min(upper, position))
        clearInterruption()
        restoring = false
        if (navigationStillValid) currentChanged(current)
        positionChanged(current.contentY, current.activeFocus ? "results" : focusKey)
        queueCapture()
    }
    function beforeModeChange() {
        capture()
        startRestore(false)
    }
    function startRestore(restoreFocus) {
        clearInterruption()
        ++restoreRevision
        restoring = true
        pendingBookmark = stateBank.get(scope, activeContext)
        pendingFocus = restoreFocus
        scheduleRestore()
    }
    function scheduleRestore() {
        const token = restoreRevision
        // Bind the queued call to this object so destruction cancels it.
        Qt.callLater(root.restore, token, 0)
    }
    function restore(token, attempt) {
        const current = view
        if (!initialized || !restoring || userInterrupted || token !== restoreRevision || !ready || !modelCoherent
                || !current || current.width <= 1 || current.height <= 1) return
        const bookmark = pendingBookmark
        const count = current.count
        let anchor = bookmark && bookmark.identity ? indexForIdentity(bookmark.identity) : -1
        if (anchor < 0 && bookmark && count > 0) anchor = Math.max(0, Math.min(count - 1, Number(bookmark.index) || 0))
        const selected = selectedIdentity ? indexForIdentity(selectedIdentity) : -1
        current.currentIndex = selected >= 0 && selected < count ? selected : -1
        if (anchor >= 0 && anchor < count) {
            current.positionViewAtIndex(anchor, ListView.Beginning)
            current.forceLayout()
            const item = current.itemAtIndex(anchor)
            if (!item && attempt < 4) { Qt.callLater(root.restore, token, attempt + 1); return }
            if (item) {
                const point = item.mapToItem(current.contentItem, 0, 0)
                const upper = current.originY + Math.max(0, current.contentHeight - current.height)
                current.contentY = Math.max(current.originY, Math.min(upper, point.y + Number(bookmark.offset || 0)))
            }
        } else {
            current.forceLayout()
            current.contentY = current.originY
        }
        if (pendingFocus) {
            const focus = bookmark ? bookmark.focus : focusKey
            if (focus === "results") current.forceActiveFocus()
            else if (focus === "search") searchFocusRequested()
        }
        pendingFocus = false
        pendingBookmark = null
        restoring = false
        positionChanged(current.contentY, current.activeFocus ? "results" : focusKey)
        queueCapture()
    }
    function changeContext() {
        if (!initialized) return
        stopEnterWindow()
        capture()
        stateBank.setDomain(scope, domain)
        activeContext = contextKey
        startRestore(false)
    }
    function beginModelChange() {
        stopEnterWindow()
        if (!restoring) {
            capture()
            startRestore(false)
        } else ++restoreRevision
        modelCoherent = false
        ++changeRevision
        ++modelRevision
    }
    Timer {
        id: modelChangeTimer
        property int revision: 0
        property bool restoreUnconditionally: false
        interval: 0
        onTriggered: {
            if (revision !== root.changeRevision) return
            root.modelCoherent = true
            root.evaluateEnter()
            if (root.userInterrupted) root.finishInterruptedInput()
            else if (restoreUnconditionally || root.restoring) root.scheduleRestore()
        }
    }
    function endModelChange() {
        modelChangeTimer.revision = ++changeRevision
        ++modelRevision
        modelChangeTimer.restoreUnconditionally = true
        modelChangeTimer.restart()
    }
    function invalidateData() {
        modelCoherent = false
        modelChangeTimer.revision = ++changeRevision
        ++modelRevision
        modelChangeTimer.restoreUnconditionally = false
        modelChangeTimer.restart()
    }
    function currentChanged(current) {
        if (restoring || !modelCoherent || !current || current !== view) return
        const index = current.currentIndex
        const token = restoreRevision
        const context = activeContext
        const revision = changeRevision
        const publish = () => {
            if (!root || root.restoring || !root.modelCoherent || root.changeRevision !== revision
                    || root.restoreRevision !== token || root.activeContext !== context
                    || root.view !== current || current.currentIndex !== index || index < 0) return
            const item = current.currentItem
            const identity = item && item.index === index && item.anchorIdentity ? item.anchorIdentity : root.identityAt(index)
            if (identity && root.indexForIdentity(identity) === index) { root.selectionRequested(identity); root.queueCapture() }
        }
        publish()
        Qt.callLater(publish)
    }
    function activateCurrent() {
        userInteracted()
        if (modelCoherent && ready && view && view.currentItem && view.currentItem.index === view.currentIndex
                && view.currentItem.anchorIdentity && indexForIdentity(view.currentItem.anchorIdentity) === view.currentIndex)
            detailsRequested(view.currentItem.anchorIdentity)
    }
    function selectIndex(index, identity) {
        userInteracted()
        if (!modelCoherent || !ready || !identity || indexForIdentity(identity) !== index) return
        if (view && index >= 0 && index < view.count) view.currentIndex = index
        if (identity) selectionRequested(identity)
        queueCapture()
    }
    function positionUpdated(current) {
        if (!restoring && modelCoherent && current === view) {
            positionChanged(current.contentY, current.activeFocus ? "results" : focusKey)
            queueCapture()
        }
    }
    function relayout() {
        if (!initialized || !view) return
        if (userInterrupted) {
            Qt.callLater(() => { if (root) root.finishInterruptedInput() })
            return
        }
        // A restore queued during loading may have run before usable geometry.
        if (restoring) { scheduleRestore(); return }
        startRestore(false)
    }
    onContextKeyChanged: changeContext()
    onDomainChanged: changeContext()
    onPresentationModeChanged: if (initialized) { stopEnterWindow(); startRestore(false) }
    onReadyChanged: {
        evaluateEnter()
        if (ready && userInterrupted) finishInterruptedInput(); else if (ready && restoring) scheduleRestore()
    }
    onVisibleChanged: {
        if (!visible) stopEnterWindow()
        else evaluateEnter()
    }
    onCountChanged: evaluateEnter()
    onOwnerEpochChanged: if (initialized) invalidateData()
    Component.onCompleted: {
        initialized = true
        stateBank.setDomain(scope, domain)
        activeContext = contextKey
        evaluateEnter()
        startRestore(true)
    }
    Component.onDestruction: {
        stopEnterWindow()
        if (ready && stateBank && ("markPageEntered" in stateBank)) stateBank.markPageEntered(scope)
        capture()
    }
    Connections {
        target: root.model
        function onModelAboutToBeReset() { root.beginModelChange() }
        function onModelReset() { root.endModelChange() }
        function onRowsAboutToBeRemoved() { root.beginModelChange() }
        function onRowsRemoved() { root.endModelChange() }
        function onRowsAboutToBeInserted() { root.beginModelChange() }
        function onRowsInserted() { root.endModelChange() }
        function onLayoutAboutToBeChanged() { root.beginModelChange() }
        function onLayoutChanged() { root.endModelChange() }
        function onDataChanged() { ++root.modelRevision }
    }
    Loader {
        id: viewLoader
        anchors.fill: parent
        sourceComponent: root.presentationMode === "grid" ? gridComponent : listComponent
        onLoaded: if (root.initialized && root.userInterrupted) root.finishInterruptedInput(); else if (root.initialized && root.restoring) root.scheduleRestore()
    }
    Component {
        id: listComponent
        ListView {
            id: list
            clip: true
            reuseItems: true
            cacheBuffer: 160
            activeFocusOnTab: true
            model: root.model
            delegate: root.delegateComponent
            currentIndex: -1
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar { onPressedChanged: if (pressed) root.userInteracted() }
            onContentYChanged: root.positionUpdated(list)
            onCurrentIndexChanged: root.currentChanged(list)
            onCurrentItemChanged: root.currentChanged(list)
            onActiveFocusChanged: if (activeFocus) root.positionUpdated(list)
            onMovementStarted: if (dragging || flicking) root.userInteracted()
            onWidthChanged: root.relayout()
            onHeightChanged: root.relayout()
            Keys.onPressed: event => { root.userInteracted(root.navigationKey(event.key)) }
            Keys.onReturnPressed: root.activateCurrent()
            Keys.onEnterPressed: root.activateCurrent()
            WheelHandler { target: null; onWheel: event => { root.userInteracted(); event.accepted = false } }
        }
    }
    Component {
        id: gridComponent
        GridView {
            id: grid
            clip: true
            reuseItems: true
            cacheBuffer: 120
            activeFocusOnTab: true
            model: root.model
            delegate: root.delegateComponent
            cellWidth: root.cellWidth
            cellHeight: root.gridHeight
            currentIndex: -1
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar { onPressedChanged: if (pressed) root.userInteracted() }
            onContentYChanged: root.positionUpdated(grid)
            onCurrentIndexChanged: root.currentChanged(grid)
            onCurrentItemChanged: root.currentChanged(grid)
            onActiveFocusChanged: if (activeFocus) root.positionUpdated(grid)
            onMovementStarted: if (dragging || flicking) root.userInteracted()
            onWidthChanged: root.relayout()
            onHeightChanged: root.relayout()
            Keys.onPressed: event => { root.userInteracted(root.navigationKey(event.key)) }
            Keys.onReturnPressed: root.activateCurrent()
            Keys.onEnterPressed: root.activateCurrent()
            WheelHandler { target: null; onWheel: event => { root.userInteracted(); event.accepted = false } }
        }
    }
}
