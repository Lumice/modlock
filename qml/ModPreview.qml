import QtQuick
import QtQuick.Effects
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Controls.impl
import Deadlock.Native
import DeadlockStyle

Rectangle {
    id: root
    property url source
    property string contextKey
    property string description: qsTr("Mod preview")
    property size decodeSize: Qt.size(960, 640)
    property real artworkOpacity: 1
    property real artworkScale: 1
    property real fallbackBottomInset: 0
    property bool artworkDesaturated: false
    property bool gallery: false
    readonly property bool remote: source.toString().startsWith("https:")
    property bool hidden: false
    // What is actually hidden: the caller's request unless the viewer chose to see mature previews.
    readonly property bool concealed: hidden && !Preferences.showMaturePreviews
    property bool active: true
    property bool allowReveal: false
    property bool compact: false
    property string placeholderTitle: ""
    readonly property bool fileFixtures: typeof uiCheckFilePreviews !== "undefined" && uiCheckFilePreviews
    readonly property bool noArtwork: !source.toString()
    // Stable hue per title so artwork-less mods are still easy to tell apart.
    readonly property real placeholderHue: {
        let h = 0
        for (let i = 0; i < placeholderTitle.length; ++i) h = (h * 31 + placeholderTitle.charCodeAt(i)) % 360
        return h / 360
    }
    property bool revealed: false
    property real imageScale: 1
    property real imageOffsetX: 0
    property real imageOffsetY: 0
    property string revealSource: ""
    property string revealContext: ""
    property bool revealHidden: false
    property bool sourceAdmitted: false
    property bool requestEnabled: false
    property bool requestInitialized: false
    property int requestRevision: 0
    property string requestSource: ""
    property string requestContext: ""
    property bool requestGallery: false
    property bool requestHidden: false
    readonly property bool revealMatches: revealed && revealSource === source.toString()
                                          && revealContext === contextKey && revealHidden === concealed
    readonly property bool revealGranted: !concealed || revealMatches
    readonly property bool admissionCurrent: sourceAdmitted && active && visible && revealGranted
                                           && requestSource === source.toString() && requestContext === contextKey
                                           && requestGallery === gallery && requestHidden === concealed
    readonly property bool masked: concealed && !revealMatches
    readonly property bool ready: admissionCurrent && image.status === Image.Ready
    readonly property real placeholderHeight: fallback.visible ? fallback.implicitHeight + (compact ? 8 : 24) : 0
    readonly property real placeholderMinimumWidth: revealButton.visible ? revealButton.implicitWidth + 8 : 0
    readonly property size imagePaintedSize: Qt.size(image.paintedWidth, image.paintedHeight)
    readonly property size imageViewportSize: Qt.size(imageArea.width, imageArea.height)
    property alias revealControl: revealButton
    // The sharp artwork item, read-only, so an effect can sample its already-admitted texture.
    // Consumers must gate on ready and must not change it.
    readonly property Item artworkItem: image
    readonly property var remoteReply: remoteLoader.status === Loader.Ready ? remoteLoader.item : null
    function updateRequest() {
        if (!requestInitialized) return
        sourceAdmitted = false
        if (remoteLoader.status === Loader.Ready) remoteLoader.item.active = false
        requestEnabled = false
        const revision = ++requestRevision
        Qt.callLater(() => {
            if (!root || !root.requestInitialized || revision !== root.requestRevision) return
            root.requestSource = root.source.toString()
            root.requestContext = root.contextKey
            root.requestGallery = root.gallery
            root.requestHidden = root.concealed
            root.sourceAdmitted = (root.requestSource.startsWith("https:") || root.requestSource.startsWith("image://localartwork/")
                                   || (root.fileFixtures && root.requestSource.startsWith("file:")))
                                  && root.active && root.visible
                                  && root.revealGranted
            root.requestEnabled = root.sourceAdmitted && root.requestSource.startsWith("https:")
        })
    }
    color: Theme.surfaceLow
    border.color: "transparent"
    radius: Theme.radius4
    clip: true
    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        visible: root.noArtwork && root.placeholderTitle !== ""
        color: Theme.surfaceHover
        Rectangle { anchors.fill: parent; radius: parent.radius; color: Qt.hsla(root.placeholderHue, .35, .5, .10) }
        ColorImage {
            anchors.centerIn: parent
            width: Math.min(parent.width, parent.height) * .85; height: width
            source: Theme.icon("sunburst"); sourceSize: Qt.size(128, 128)
            color: Theme.brass; opacity: .06
        }
        Label {
            anchors.centerIn: parent
            text: root.placeholderTitle.trim().charAt(0).toUpperCase()
            color: Theme.secondary
            font.family: Theme.displayFamily
            font.pointSize: Math.max(Theme.body, Math.min(parent.height * 0.32, 48))
            font.weight: Font.Bold
            Accessible.ignored: true
        }
    }
    Accessible.role: Accessible.Graphic
    Accessible.name: description
    Accessible.description: !source.toString() ? qsTr("No preview available") : ""
    property real desaturateProgress: root.artworkDesaturated ? 1 : 0
    Behavior on desaturateProgress {
        enabled: !Tokens.reduceMotion && Tokens.normal > 0 && root.ready
        NumberAnimation { id: desaturateAnim; duration: Tokens.normal; easing.type: Easing.OutCubic }
    }
    function snapArtworkTransition() {
        if (opacityAnim.running) opacityAnim.stop()
        if (desaturateAnim.running) desaturateAnim.stop()
        image._displayOpacity = Qt.binding(() => root.artworkOpacity)
        root.desaturateProgress = Qt.binding(() => root.artworkDesaturated ? 1 : 0)
    }
    onSourceChanged: { revealed = false; snapArtworkTransition(); updateRequest() }
    onContextKeyChanged: { revealed = false; snapArtworkTransition(); updateRequest() }
    onConcealedChanged: { revealed = false; updateRequest() }
    onActiveChanged: { if (!active) revealed = false; updateRequest() }
    onVisibleChanged: updateRequest()
    onGalleryChanged: updateRequest()
    onRevealedChanged: {
        if (revealed) {
            revealSource = source.toString()
            revealContext = contextKey
            revealHidden = concealed
        } else {
            revealSource = ""
            revealContext = ""
        }
        updateRequest()
    }
    Component.onCompleted: { requestInitialized = true; updateRequest() }
    Connections {
        target: Tokens
        function onReduceMotionChanged() { if (Tokens.reduceMotion) root.snapArtworkTransition() }
    }

    Loader {
        id: remoteLoader
        active: root.requestEnabled
        sourceComponent: PreviewStatus {
            Component.onCompleted: {
                source = root.requestSource
                contextKey = root.requestContext
                gallery = root.requestGallery
                active = root.requestEnabled && root.admissionCurrent
            }
        }
    }
    Item {
        id: imageArea
        anchors.fill: parent
        anchors.margins: 1
        clip: true
        Image {
            id: image
            property real _displayOpacity: root.artworkOpacity
            Behavior on _displayOpacity {
                enabled: !Tokens.reduceMotion && Tokens.normal > 0 && root.ready
                NumberAnimation { id: opacityAnim; duration: Tokens.normal; easing.type: Easing.OutCubic }
            }
            opacity: _displayOpacity
            layer.enabled: root.desaturateProgress > 0.001 && GraphicsInfo.api !== GraphicsInfo.Software
            layer.effect: MultiEffect { saturation: -0.85 * root.desaturateProgress }
            objectName: "previewImage"
            width: imageArea.width
            height: imageArea.height
            x: root.imageOffsetX
            y: root.imageOffsetY
            scale: root.imageScale * root.artworkScale
            transformOrigin: Item.Center
            source: root.admissionCurrent
                    ? (root.requestSource.startsWith("https:")
                       ? root.requestEnabled && root.remoteReply ? root.remoteReply.imageSource : ""
                       : root.requestSource.startsWith("image://localartwork/") || (root.fileFixtures && root.requestSource.startsWith("file:")) ? root.requestSource : "") : ""
            sourceSize: root.remote ? (root.gallery ? Qt.size(Math.min(root.decodeSize.width, 1920), Math.min(root.decodeSize.height, 1080)) : Qt.size(Math.min(root.decodeSize.width, 530), Math.min(root.decodeSize.height, 350))) : root.decodeSize
            cache: !root.remote
            asynchronous: true
            fillMode: root.compact ? Image.PreserveAspectCrop : Image.PreserveAspectFit
            visible: root.ready
            Accessible.ignored: true
        }
    }
    ColumnLayout {
        id: fallback
        anchors.centerIn: parent
        anchors.verticalCenterOffset: -Math.max(0, Math.min(root.fallbackBottomInset, root.height - implicitHeight - 16)) / 2
        width: Math.max(0, parent.width - (root.compact ? 8 : 24))
        spacing: 8
        visible: !root.ready && !(root.noArtwork && root.placeholderTitle !== "")
        Label {
            objectName: "previewFallback"
            Layout.fillWidth: true
            text: !root.source.toString() ? qsTr("No preview available")
                : root.masked ? qsTr("Preview hidden")
                : root.sourceAdmitted && root.remote && root.remoteReply && root.remoteReply.state === PreviewStatus.Busy ? qsTr("Artwork busy; waiting for capacity…")
                : (root.sourceAdmitted && root.remote && root.remoteReply && root.remoteReply.state === PreviewStatus.Unavailable) || (root.sourceAdmitted && image.status === Image.Error) ? qsTr("Preview unavailable") : qsTr("Loading preview…")
            textFormat: Text.PlainText
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            color: Theme.secondary
            font.pointSize: root.compact ? Theme.small : Theme.body
        }
        Button {
            id: revealButton
            objectName: "revealPreview"
            Layout.alignment: Qt.AlignHCenter
            visible: root.allowReveal && root.masked && root.source.toString() !== ""
            text: qsTr("Show preview")
            onClicked: root.revealed = true
        }
    }
}
