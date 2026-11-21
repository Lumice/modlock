pragma ComponentBehavior: Bound
import QtQuick
import Deadlock.Native

Item {
    id: root
    required property bool valid
    property var snapshot: null
    property Item owner: null
    property Item invoker: null
    property int currentIndex: 0
    property string title: snapshot ? String(snapshot.title || "") : ""
    property bool requested: false
    readonly property bool opened: requested && valid
    signal indexSelected(int index)
    signal imageRevealed(int index, string source, string contextKey)
    signal focusRequested(var state, var owner, var invoker, int index)

    function sameSources(a, b) {
        if (!a || !b || a.length !== b.length) return false
        for (let i = 0; i < a.length; ++i) if (String(a[i]) !== String(b[i])) return false
        return true
    }
    function slots(images, source, preferredIndex) {
        const sources = []
        for (let i = 0; i < images.length; ++i) sources.push(String(images[i]))
        let index = preferredIndex >= 0 && preferredIndex < sources.length && sources[preferredIndex] === source
                    ? preferredIndex : sources.indexOf(source)
        if (source && index < 0) { sources.unshift(source); index = 0 }
        return {sources: sources, index: index}
    }
    function open(state, preview, target, item = null) {
        if (requested || !preview || !preview.ready || preview.masked) return
        const sources = []
        for (let i = 0; i < state.sources.length; ++i) sources.push(String(state.sources[i]))
        if (state.startIndex < 0 || state.startIndex >= sources.length) return
        owner = item
        invoker = target
        currentIndex = state.startIndex
        // Activate after the host validity binding sees the complete snapshot.
        snapshot = Object.assign({}, state, {sources: sources, seedSource: preview.revealMatches ? state.source : ""})
        requested = true
    }
    function restoreFocus(state, previousOwner, target, index) {
        if (!requested) focusRequested(state, previousOwner, target, index)
    }
    function finish() {
        if (!requested) return
        const state = snapshot
        const previousOwner = owner
        const target = invoker
        const index = currentIndex
        requested = false
        snapshot = null
        owner = null
        invoker = null
        Qt.callLater(root.restoreFocus, state, previousOwner, target, index)
    }
    function close() {
        if (viewerLoader.status === Loader.Ready) (viewerLoader.item as MediaViewer).close()
        else finish()
    }
    onRequestedChanged: if (requested && !valid) finish()
    onOpenedChanged: if (!opened && requested) finish()
    Loader {
        id: viewerLoader
        active: root.opened
        sourceComponent: MediaViewer {
            sources: root.snapshot ? root.snapshot.sources : []
            contextKey: root.snapshot ? root.snapshot.token : ""
            currentIndex: root.snapshot ? root.snapshot.startIndex : 0
            title: root.title
            hidden: root.snapshot ? root.snapshot.hidden : false
            active: root.opened
            revealSeedIndex: root.snapshot && root.snapshot.seedSource ? root.snapshot.startIndex : -1
            revealSeedSource: root.snapshot ? root.snapshot.seedSource : ""
            revealSeedContextKey: root.snapshot ? root.snapshot.token : ""
            onIndexSelected: index => {
                if (!root.opened) return
                root.indexSelected(index)
                if (root.opened) root.currentIndex = index
            }
            onImageRevealed: (index, source, contextKey) => {
                if (root.opened && index === root.currentIndex && contextKey === root.snapshot.token)
                    root.imageRevealed(index, source, contextKey)
            }
            onClosed: root.finish()
        }
    }
}
