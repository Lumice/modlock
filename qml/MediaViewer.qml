pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    required property var sources
    required property string contextKey
    required property int currentIndex
    required property bool hidden
    readonly property bool concealed: hidden && !Preferences.showMaturePreviews
    required property bool active
    property int revealSeedIndex: -1
    property string revealSeedSource: ""
    property string revealSeedContextKey: ""
    property real zoom: 1
    property real panX: 0
    property real panY: 0
    property real navigationWheel: 0
    property real zoomWheel: 0
    property bool initialized: false
    signal indexSelected(int index)
    signal imageRevealed(int index, string source, string contextKey)

    function clampPan() {
        const size = preview.imagePaintedSize
        const viewport = preview.imageViewportSize
        const maxX = Math.max(0, (size.width * zoom - viewport.width) / 2)
        const maxY = Math.max(0, (size.height * zoom - viewport.height) / 2)
        panX = isFinite(panX) ? Math.max(-maxX, Math.min(maxX, panX)) : 0
        panY = isFinite(panY) ? Math.max(-maxY, Math.min(maxY, panY)) : 0
    }
    function setZoom(value, x, y) {
        const next = isFinite(value) ? Math.max(1, Math.min(4, value)) : 1
        const centerX = preview.width / 2
        const centerY = preview.height / 2
        const pointerX = Math.max(0, Math.min(preview.width, x)) - centerX
        const pointerY = Math.max(0, Math.min(preview.height, y)) - centerY
        const ratio = next / zoom
        panX = pointerX - (pointerX - panX) * ratio
        panY = pointerY - (pointerY - panY) * ratio
        zoom = next
        clampPan()
    }
    function fit() { zoom = 1; panX = 0; panY = 0 }
    function changeIndex(delta) {
        const next = Math.max(0, Math.min(sources.length - 1, currentIndex + delta))
        if (next === currentIndex) return
        preview.revealed = false
        currentIndex = next
    }
    function handleWheel(event, item) {
        event.accepted = true
        const delta = event.angleDelta.y !== 0 ? event.angleDelta.y / 120 : event.pixelDelta.y / 40
        const point = preview.mapFromItem(item, event.x, event.y)
        if (event.modifiers & Qt.ControlModifier) {
            zoomWheel += delta
            const steps = zoomWheel > 0 ? Math.floor(zoomWheel) : Math.ceil(zoomWheel)
            zoomWheel -= steps
            if (steps) setZoom(zoom * Math.pow(1.2, Math.max(-16, Math.min(16, steps))), point.x, point.y)
        } else {
            navigationWheel += delta
            const steps = navigationWheel > 0 ? Math.floor(navigationWheel) : Math.ceil(navigationWheel)
            navigationWheel -= steps
            if (steps) changeIndex(-steps)
        }
    }
    function moveFocus(backward) {
        const controls = [preview.revealControl, previousButton, nextButton, zoomOutButton,
                          zoomInButton, fitButton, closeButton].filter(control => control.visible && control.enabled)
        if (!controls.length) return
        let index = controls.findIndex(control => control.activeFocus)
        index = index < 0 ? (backward ? 0 : -1) : index
        controls[(index + (backward ? controls.length - 1 : 1)) % controls.length].forceActiveFocus()
    }
    onCurrentIndexChanged: {
        if (!initialized) return
        preview.revealed = false
        panX = 0
        panY = 0
        indexSelected(currentIndex)
    }
    onContextKeyChanged: if (initialized) close()
    onSourcesChanged: if (initialized) close()
    onConcealedChanged: if (initialized) close()
    onActiveChanged: if (initialized && !active) close()

    parent: Overlay.overlay
    width: Math.max(1, Math.min(parent ? parent.width - 32 : 1000, 1200))
    height: Math.max(1, Math.min(parent ? parent.height - 32 : 720, 850))
    x: parent ? (parent.width - width) / 2 : 0
    y: parent ? (parent.height - height) / 2 : 0
    modal: true
    focus: true
    closePolicy: Popup.CloseOnPressOutside
    Overlay.modal: Rectangle {
        color: Qt.alpha(Theme.window, 0.72)
        WheelHandler { target: null; acceptedModifiers: Qt.KeyboardModifierMask; acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad; onWheel: event => event.accepted = true }
    }
    background: Rectangle {
        id: viewerBackground
        color: Theme.window
        border.color: Theme.rule
        WheelHandler { target: null; acceptedModifiers: Qt.KeyboardModifierMask; acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad; onWheel: event => root.handleWheel(event, viewerBackground) }
    }
    header: Label {
        id: heading
        padding: 12
        text: root.title
        textFormat: Text.PlainText
        elide: Text.ElideRight
        font.weight: Font.Medium
        WheelHandler { target: null; acceptedModifiers: Qt.KeyboardModifierMask; acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad; onWheel: event => root.handleWheel(event, heading) }
    }
    Component.onCompleted: {
        if (concealed && revealSeedIndex === currentIndex && revealSeedContextKey === contextKey
                && revealSeedSource === String(sources[currentIndex] || "")) preview.revealed = true
        initialized = true
        open()
    }
    onOpened: fitButton.forceActiveFocus()

    contentItem: FocusScope {
        id: content
        implicitWidth: 800
        implicitHeight: 600
        focus: true
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: event => {
            if (event.key === Qt.Key_Escape) { event.accepted = true; root.close(); return }
            else if (event.key === Qt.Key_Left) root.changeIndex(-1)
            else if (event.key === Qt.Key_Right) root.changeIndex(1)
            else if (event.key === Qt.Key_Plus || event.key === Qt.Key_Equal) root.setZoom(root.zoom * 1.2, preview.width / 2, preview.height / 2)
            else if (event.key === Qt.Key_Minus) root.setZoom(root.zoom / 1.2, preview.width / 2, preview.height / 2)
            else if (event.key === Qt.Key_0) root.fit()
            else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) root.moveFocus(event.key === Qt.Key_Backtab || !!(event.modifiers & Qt.ShiftModifier))
            else return
            event.accepted = true
        }
        WheelHandler { target: null; acceptedModifiers: Qt.KeyboardModifierMask; acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad; onWheel: event => root.handleWheel(event, content) }
        ColumnLayout {
            anchors.fill: parent
            spacing: 8
            ModPreview {
                id: preview
                objectName: "mediaViewerPreview"
                Layout.fillWidth: true
                Layout.fillHeight: true
                source: root.sources[root.currentIndex] || ""
                contextKey: JSON.stringify([root.contextKey, root.currentIndex])
                description: root.title
                hidden: root.hidden
                active: root.active && root.visible
                allowReveal: true
                gallery: true
                decodeSize: Qt.size(1920, 1080)
                imageScale: root.zoom
                imageOffsetX: root.panX
                imageOffsetY: root.panY
                onImagePaintedSizeChanged: root.clampPan()
                onImageViewportSizeChanged: root.clampPan()
                onRevealedChanged: {
                    if (!revealed || !root.initialized) return
                    const index = root.currentIndex
                    const source = String(root.sources[index] || "")
                    const context = root.contextKey
                    Qt.callLater(() => {
                        if (root && preview && root.active && root.visible && root.currentIndex === index && root.contextKey === context
                                && String(root.sources[index] || "") === source && preview.revealMatches)
                            root.imageRevealed(index, source, context)
                    })
                }
                DragHandler {
                    id: drag
                    target: null
                    acceptedButtons: Qt.LeftButton
                    enabled: preview.ready && root.zoom > 1
                    property real startX: 0
                    property real startY: 0
                    property bool pressedImage: false
                    onActiveChanged: {
                        if (!active) { pressedImage = false; return }
                        startX = root.panX
                        startY = root.panY
                        const point = centroid.pressPosition
                        pressedImage = Math.abs(point.x - preview.width / 2 - startX) <= preview.imagePaintedSize.width * root.zoom / 2
                                       && Math.abs(point.y - preview.height / 2 - startY) <= preview.imagePaintedSize.height * root.zoom / 2
                    }
                    onTranslationChanged: if (active && pressedImage) {
                        root.panX = startX + activeTranslation.x
                        root.panY = startY + activeTranslation.y
                        root.clampPan()
                    }
                }
            }
            Flow {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                spacing: 6
                Button { id: previousButton; text: qsTr("Previous"); enabled: root.currentIndex > 0; onClicked: root.changeIndex(-1) }
                Label { padding: 8; text: qsTr("%1 of %2").arg(root.currentIndex + 1).arg(root.sources.length); textFormat: Text.PlainText }
                Button { id: nextButton; text: qsTr("Next"); enabled: root.currentIndex + 1 < root.sources.length; onClicked: root.changeIndex(1) }
                Button { id: zoomOutButton; text: qsTr("Zoom out"); enabled: root.zoom > 1; onClicked: root.setZoom(root.zoom / 1.2, preview.width / 2, preview.height / 2) }
                Button { id: zoomInButton; text: qsTr("Zoom in"); enabled: root.zoom < 4; onClicked: root.setZoom(root.zoom * 1.2, preview.width / 2, preview.height / 2) }
                Button { id: fitButton; text: qsTr("Fit"); onClicked: root.fit() }
                Label { padding: 8; text: qsTr("%1% of fit").arg(Math.round(root.zoom * 100)); textFormat: Text.PlainText }
                Button { id: closeButton; objectName: "closeEnlargedPreview"; text: qsTr("Close"); onClicked: root.close() }
            }
        }
    }
}
