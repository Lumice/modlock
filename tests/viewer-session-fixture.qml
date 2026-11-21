pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Deadlock.Native

Item {
    id: probe
    required property var library
    required property LibraryModel focusLibrary
    required property CatalogController catalogController
    required property string firstImage
    required property string secondImage
    required property string encodedImage
    property string host: "results"
    property string mode: "list"
    property string scope: "library"
    property var localMod: ({id: "local-a", title: "Local", images: [firstImage, secondImage], previewHidden: false})
    property var remoteDetail: ({state: "ready", summary: {identity: "mod:100", title: "Remote", images: [firstImage, secondImage], masked: false}})
    property var collectionDetail: ({state: "ready", header: {collectionId: "7", name: "Collection", images: [firstImage, secondImage], isNsfw: false, contentRatingKnown: true}})
    property alias result: results
    property alias local: local
    property alias remote: remote
    property alias collection: collection
    property alias rows: rows
    property alias libraryPage: libraryPage
    property var activeHost: host === "local" ? local : host === "remote" ? remote : host === "collection" ? collection : results
    width: 1100; height: 740; z: 100000
    MediaViewState { id: bank }
    ListModel { id: rows }
    function resetRows() {
        rows.clear()
        for (let i = 0; i < 60; ++i) rows.append({identity: "row-" + i, source: firstImage, title: "Row " + i, hidden: false, gallery: JSON.stringify([firstImage, secondImage])})
        results.selectedIdentity = "row-0"
    }
    function snapshot(identity) {
        const index = Number(identity.slice(4))
        if (index < 0 || index >= rows.count) return null
        const row = rows.get(index)
        const images = JSON.parse(row.gallery)
        return {identity: row.identity, source: row.source, title: row.title, hidden: row.hidden, sources: images}
    }
    MediaResultView {
        id: results
        visible: probe.host === "results"
        width: probe.width; height: probe.height
        stateBank: bank; model: rows; scope: probe.scope
        contextKey: "fixture"; domain: "fixture"; selectedIdentity: "row-0"; focusKey: "results"
        presentationMode: probe.mode
        indexForIdentity: identity => Number(identity.slice(4))
        identityAt: index => index >= 0 && index < rows.count ? rows.get(index).identity : ""
        snapshotFor: (identity, revision) => probe.snapshot(identity)
        onSelectionRequested: identity => selectedIdentity = identity
        delegateComponent: Component {
            MediaCard {
                required property string identity
                index: model.index
                required property var model
                anchorIdentity: identity
                resultHost: results
                heading: ownerSnapshot ? ownerSnapshot.title : ""
                ownerSnapshot: { const revision = results.modelRevision; return probe.snapshot(identity) }
            }
        }
    }
    ModDetails {
        id: local
        visible: probe.host === "local"
        width: probe.width; height: probe.height
        mod: probe.localMod; readOnly: true
        mediaContextKey: JSON.stringify([mod.id || "", "fixture"])
    }
    CatalogDetails {
        id: remote
        visible: probe.host === "remote"
        width: probe.width; height: probe.height
        controller: probe.catalogController; remote: probe.library.remote
    }
    CollectionDetails {
        id: collection
        visible: probe.host === "collection"
        width: probe.width; height: probe.height
        controller: probe.catalogController
    }
    Loader {
        id: libraryPage
        active: probe.host === "library-page"
        width: 820; height: probe.height
        sourceComponent: LibraryPage {
            library: probe.focusLibrary
            viewState: bank
            fontFocusReturnAllowed: true
        }
    }
    Component.onCompleted: resetRows()
}
