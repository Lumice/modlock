pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

Item {
    id: root
    required property int index
    required property string anchorIdentity
    required property var resultHost
    required property var ownerSnapshot
    required property string heading
    property string subtitle: ""
    property bool twoLineTitle: false
    property string updateLabel: ""
    property string healthLabel: ""
    property string healthTone: "muted"
    property string status: ""
    property string statusTone: ""
    property bool selected: false
    property string actionKind: ""
    property string actionText: ""
    property string actionName: ""
    property bool actionVisible: true
    property bool actionEnabled: false
    property bool actionChecked: false
    property bool installVisible: false
    property bool installedToggle: false
    property bool installedOn: false
    property string installedProfile: ""
    property string statsText: ""
    property var catalogStats: []
    // Multi-select: checkbox shown on hover or while a selection exists.
    property bool selectable: false
    property bool marked: false
    property bool markMode: false
    signal markToggled(bool marked)
    property bool installEnabled: false
    property bool installUpdate: false
    property string installText: qsTr("Download")
    property bool pooled: false
    property bool initialized: false
    property string mediaToken: ""
    property var mediaDemandOwner: null
    property string lastDemandedIdentity: ""
    property alias preview: preview
    // Browse sound items replace the artwork with an inline preview player.
    property bool soundItem: false
    property string audioUrl: ""
    property real enterOpacity: 1
    property real enterOffsetY: 0
    property int lastEnterGeneration: 0
    property int _identityEpoch: 0
    property string _toggleTrackedIdentity: ""
    property bool _toggleTrackedSwitchedOff: false
    property bool _toggleIdentitySettled: false
    opacity: enterOpacity
    transform: Translate { id: enterShift; y: root.enterOffsetY }
    readonly property bool switchedOff: (actionKind === "switch" && !actionChecked) || (installedToggle && !installedOn)
    readonly property bool joined: resultHost.scope === "library" && resultHost.presentationMode === "list"
    readonly property bool stacked: resultHost.presentationMode === "grid" || resultHost.width < 410
    // Grid mode draws a poster: the art fills the card and the text sits over a scrim.
    // Library keeps its switch on the art; Browse keeps a slim footer for stats and actions.
    readonly property bool poster: resultHost.presentationMode === "grid"
    readonly property bool posterFooter: poster && resultHost.scope !== "library"
    readonly property real posterFooterHeight: resultHost.posterFooterHeight || 52
    readonly property bool lifted: poster && cardHover.hovered && preview.ready && !Tokens.reduceMotion
    readonly property bool inRange: { const position = root.x + root.y; return resultHost.inRange(preview) }
    readonly property bool ownerCurrent: !!ownerSnapshot && ownerSnapshot.identity === anchorIdentity
    readonly property bool mediaActive: initialized && visible && !pooled && inRange && ownerCurrent
    readonly property bool interactable: visible && !pooled && resultHost.modelCoherent && resultHost.ready
                                         && ownerSnapshot && ownerSnapshot.identity === anchorIdentity
    signal detailsRequested()
    signal actionRequested()
    signal installRequested()
    signal toggleRequested(bool enabled)
    signal mediaDemandRequested(string consumer, string identity, bool wanted)

    width: Math.max(1, resultHost.cellWidth - (joined ? 0 : 8))
    height: resultHost.presentationMode === "grid" ? Math.max(1, resultHost.gridHeight - 8)
            : Math.max(resultHost.listHeight - (joined ? 0 : 8), cardLayout.implicitHeight + Theme.spacing16)
    // Select first: the card's own TapHandler selects on the same click, and a
    // selection change after opening invalidates the viewer (it flashed shut).
    function openLarger() {
        if (!interactable) return
        resultHost.selectIndex(root.index, root.anchorIdentity)
        resultHost.openViewer(root, largerButton)
    }
    function snapEnter() {
        enterAnim.stop()
        enterOpacity = 1
        enterOffsetY = 0
    }
    function tryEnter() {
        if (!resultHost || !("enterArmed" in resultHost) || !resultHost.enterArmed) return
        const gen = Number(resultHost.enterGeneration || 0)
        if (gen <= 0 || lastEnterGeneration === gen) return
        lastEnterGeneration = gen
        if (root.index >= 0 && root.index < 12 && !Tokens.reduceMotion && Tokens.normal > 0 && !root.pooled && root.visible) {
            enterOpacity = 0
            enterOffsetY = 6
            enterPause.duration = root.index * 25
            enterAnim.restart()
        } else {
            snapEnter()
        }
    }
    function withdrawDemand() {
        if (!lastDemandedIdentity) return
        const identity = lastDemandedIdentity
        lastDemandedIdentity = ""
        mediaDemandRequested("card:" + mediaToken, identity, false)
    }
    function syncDemand(revalidate = false) {
        const wanted = mediaActive && !!mediaDemandOwner
        if (lastDemandedIdentity && (!wanted || lastDemandedIdentity !== anchorIdentity))
            withdrawDemand()
        if (wanted && (!lastDemandedIdentity || revalidate)) {
            lastDemandedIdentity = anchorIdentity
            // Share the existing bounded per-card key: row IDs can exceed the
            // consumer limit, and two live cards must withdraw independently.
            mediaDemandRequested("card:" + mediaToken, lastDemandedIdentity, true)
        }
    }
    function focusTitle() {
        if (!interactable || resultHost.viewerOpen || !titleButton.visible || !titleButton.enabled) return false
        titleButton.forceActiveFocus()
        return true
    }
    function resetToggleTracking() {
        ++_identityEpoch
        _toggleIdentitySettled = false
        toggleFlashAnim.stop()
        toggleFlashRing.opacity = 0
        _toggleTrackedIdentity = anchorIdentity
        _toggleTrackedSwitchedOff = switchedOff
        const epoch = _identityEpoch
        Qt.callLater(() => {
            if (!root || root._identityEpoch !== epoch) return
            root._toggleTrackedIdentity = root.anchorIdentity
            root._toggleTrackedSwitchedOff = root.switchedOff
            root._toggleIdentitySettled = root.initialized && !root.pooled && root.visible
        })
    }
    onOwnerSnapshotChanged: if (resultHost.ready && resultHost.modelCoherent) syncDemand(true)
    // Library IDs can exceed PreviewStatus's 1,024-byte context limit.
    onAnchorIdentityChanged: {
        withdrawDemand()
        if (initialized) mediaToken = resultHost.issueToken()
        resetToggleTracking()
        syncDemand()
    }
    onSwitchedOffChanged: {
        if (!initialized || pooled || !visible || !_toggleIdentitySettled) {
            if (!_toggleIdentitySettled) {
                _toggleTrackedIdentity = anchorIdentity
                _toggleTrackedSwitchedOff = switchedOff
            } else {
                resetToggleTracking()
            }
            return
        }
        const wasOff = _toggleTrackedSwitchedOff
        const id = _toggleTrackedIdentity
        const epoch = _identityEpoch
        Qt.callLater(() => {
            if (!root) return
            const sameItem = root._identityEpoch === epoch
                && root._toggleIdentitySettled
                && root.anchorIdentity === id
                && root.initialized
                && !root.pooled
                && root.visible
            root._toggleTrackedIdentity = root.anchorIdentity
            root._toggleTrackedSwitchedOff = root.switchedOff
            if (sameItem && wasOff && !root.switchedOff && !Tokens.reduceMotion && Tokens.normal > 0)
                toggleFlashAnim.restart()
        })
    }
    onMediaActiveChanged: syncDemand()
    onMediaDemandOwnerChanged: syncDemand()
    onPooledChanged: {
        if (pooled) snapEnter()
        resetToggleTracking()
    }
    onVisibleChanged: {
        if (!visible) {
            snapEnter()
            _toggleIdentitySettled = false
            toggleFlashAnim.stop()
            toggleFlashRing.opacity = 0
        } else if (initialized && !pooled) {
            resetToggleTracking()
        }
    }
    Component.onCompleted: {
        mediaToken = resultHost.issueToken()
        initialized = true
        resetToggleTracking()
        if (resultHost && typeof resultHost.evaluateEnter === "function") resultHost.evaluateEnter()
        tryEnter()
    }
    Component.onDestruction: withdrawDemand()
    ListView.onPooled: pooled = true
    ListView.onReused: pooled = false
    GridView.onPooled: pooled = true
    GridView.onReused: pooled = false
    SequentialAnimation {
        id: enterAnim
        PauseAnimation { id: enterPause; duration: 0 }
        ParallelAnimation {
            NumberAnimation { target: root; property: "enterOffsetY"; from: 6; to: 0; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutCubic }
            NumberAnimation { target: root; property: "enterOpacity"; from: 0; to: 1; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutCubic }
        }
    }
    SequentialAnimation {
        id: toggleFlashAnim
        objectName: "toggleFlashAnimation"
        onRunningChanged: if (!running) toggleFlashRing.opacity = 0
        PropertyAction { target: toggleFlashRing; property: "opacity"; value: 0 }
        NumberAnimation { target: toggleFlashRing; property: "opacity"; from: 0; to: 1; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutCubic }
        NumberAnimation { target: toggleFlashRing; property: "opacity"; from: 1; to: 0; duration: Math.max(1, Tokens.normal); easing.type: Easing.InCubic }
    }
    Connections {
        target: Tokens
        function onReduceMotionChanged() {
            if (Tokens.reduceMotion) {
                root.snapEnter()
                toggleFlashAnim.stop()
                toggleFlashRing.opacity = 0
            }
        }
    }
    Connections {
        target: root.resultHost
        ignoreUnknownSignals: true
        function onModelCoherentChanged() { if (root.resultHost.ready && root.resultHost.modelCoherent) root.syncDemand(true) }
        function onReadyChanged() { if (root.resultHost.ready && root.resultHost.modelCoherent) root.syncDemand(true) }
        function onEnterArmedChanged() { root.tryEnter() }
    }
    Connections {
        target: root.mediaDemandOwner
        ignoreUnknownSignals: true
        function onLibraryMediaCapacityChanged() {
            if (root.mediaActive && root.lastDemandedIdentity === root.anchorIdentity)
                root.syncDemand(true)
        }
    }
    HoverHandler { id: cardHover; objectName: "cardHoverHandler" }
    // Clicking anywhere on the card selects it; double-click opens details.
    TapHandler {
        enabled: root.interactable
        onTapped: { root.resultHost.selectIndex(root.index, root.anchorIdentity) }
        onDoubleTapped: { root.resultHost.userInteracted(); root.detailsRequested() }
    }
    Translate {
        id: liftShift
        y: root.lifted ? -3 : 0
        Behavior on y { enabled: preview.ready; NumberAnimation { duration: Theme.motionFast; easing.type: Easing.OutCubic } }
    }
    Rectangle {
        id: cardFrame
        transform: liftShift
        anchors.fill: parent
        radius: root.joined ? 0 : Theme.radiusCard
        color: root.selected ? Theme.accentSoft : cardHover.hovered ? Theme.surfaceHover : Theme.surface
        border.width: root.joined ? 0 : root.selected ? 2 : 1
        border.color: root.selected ? Theme.accentRing : cardHover.hovered ? Theme.ruleStrong : Theme.rule
        Behavior on color { ColorAnimation { duration: Theme.motionFast } }
    }
    Rectangle {
        anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
        width: 3; color: Theme.accentRing; visible: root.joined && root.selected
    }
    Rectangle {
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        height: 1; color: Theme.rule; visible: root.joined
    }
    Item {
        id: artBox
        transform: liftShift
        x: root.poster ? 0 : cardLayout.x + artSlot.x
        y: root.poster ? 0 : cardLayout.y + artSlot.y
        width: root.poster ? root.width : artSlot.width
        height: root.poster ? Math.max(1, root.height - (root.posterFooter ? root.posterFooterHeight : 0)) : artSlot.height
        clip: true
        ModPreview {
            id: preview
            objectName: "rowPreview"
            anchors.fill: parent
            radius: root.poster ? 0 : root.stacked ? Theme.radiusCard : Theme.radius4
            artworkOpacity: root.switchedOff ? .55 : 1
            artworkDesaturated: root.switchedOff
            scale: !root.poster && cardHover.hovered ? 1.035 : 1
            Behavior on scale { NumberAnimation { duration: Theme.motionNormal } }
            artworkScale: root.poster && cardHover.hovered && preview.ready && !Tokens.reduceMotion ? 1.05 : 1
            Behavior on artworkScale { NumberAnimation { duration: Theme.motionNormal } }
            fallbackBottomInset: root.poster ? Math.max(0, artBox.height - (cardLayout.y + textColumn.y + titleButton.y - Theme.spacing8)) : 0
            visible: !root.soundItem
            source: root.ownerSnapshot && !root.soundItem ? root.ownerSnapshot.source : ""
            hidden: !root.ownerSnapshot || !!root.ownerSnapshot.hidden
            contextKey: root.mediaToken
            description: qsTr("Preview of %1").arg(root.heading)
            placeholderTitle: root.heading
            decodeSize: Qt.size(530, 350)
            gallery: false
            active: root.mediaActive && !root.soundItem
            enabled: root.interactable
            allowReveal: true
            compact: true
        }
        // Bottom scrim so the poster title reads over any art; Theme.surface keeps it right in both themes.
        Rectangle {
            visible: root.poster && !root.soundItem
            anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
            height: parent.height * (root.posterFooter ? .6 : .66)
            gradient: Gradient {
                GradientStop { position: 0; color: Qt.alpha(Theme.surface, 0) }
                GradientStop { position: .45; color: Qt.alpha(Theme.surface, .72) }
                GradientStop { position: 1; color: Qt.alpha(Theme.surface, .97) }
            }
        }
        // In poster mode the title block is not part of the art's click area.
        Item {
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            height: root.poster ? Math.max(0, Math.min(parent.height, cardLayout.y + textColumn.y + titleButton.y - 4)) : parent.height
            TapHandler { enabled: root.interactable && preview.ready; onTapped: root.openLarger() }
            HoverHandler { id: previewHover; enabled: root.interactable && preview.ready; cursorShape: Qt.PointingHandCursor }
        }
        Loader {
            id: soundPanel
            anchors.fill: parent
            active: root.soundItem
            visible: active
            sourceComponent: SoundPreview {
                identity: root.anchorIdentity
                heading: root.heading
                audioUrl: root.audioUrl
                // The same owner-specific rule as artwork: hidden or unrated
                // content needs the mature preference or an explicit reveal.
                concealed: !root.ownerCurrent || !root.ownerSnapshot || (!!root.ownerSnapshot.hidden && !Preferences.showMaturePreviews)
                interactive: root.interactable
                compact: !root.stacked
                radius: root.poster ? 0 : root.stacked ? Theme.radiusCard : Theme.radius4
                controlsHeight: root.poster ? Math.max(0, Math.min(artBox.height, cardLayout.y + textColumn.y + titleButton.y - 4)) : artBox.height
            }
        }
        CheckBox {
            id: markBox
            enabled: root.interactable
            objectName: "markBox"
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.margins: root.poster ? Theme.spacing8 : 2
            // The current card always shows it so keyboard users can start a selection.
            visible: root.selectable && (root.markMode || root.marked || cardHover.hovered || activeFocus)
            checked: root.marked
            width: root.poster ? 32 : implicitWidth
            height: root.poster ? 32 : implicitHeight
            padding: root.poster ? 7 : 8
            Accessible.name: qsTr("Select %1").arg(root.heading)
            background: Rectangle {
                radius: root.poster ? Theme.radius : 5
                color: root.poster
                    ? (markBox.hovered || markBox.visualFocus || markBox.checked ? Qt.alpha(Theme.surface, .90) : Qt.alpha(Theme.surface, .74))
                    : Theme.surface
                border.width: root.poster ? 1 : 0
                border.color: markBox.checked ? Theme.accentRing : markBox.hovered ? Theme.ruleStrong : Theme.rule
            }
            onToggled: { root.markToggled(checked); checked = Qt.binding(() => root.marked) }
        }
        Label {
            objectName: "posterOffTag"
            // The switch's accessible name already says the mod is off.
            Accessible.ignored: true
            visible: root.poster && root.switchedOff
            anchors.top: parent.top; anchors.topMargin: 9
            anchors.left: markBox.visible ? markBox.right : parent.left
            anchors.leftMargin: markBox.visible ? 4 : 9
            text: qsTr("Off")
            textFormat: Text.PlainText
            color: Theme.secondary
            font.pointSize: Theme.small
            font.weight: Font.DemiBold
            leftPadding: 7; rightPadding: 7; topPadding: 1; bottomPadding: 1
            background: Rectangle { radius: Theme.radius4; color: Qt.alpha(Theme.surface, .82); border.width: 1; border.color: Theme.ruleStrong }
        }
        ToolButton {
            id: largerButton
            objectName: "largerButton"
            anchors.right: parent.right
            anchors.top: root.poster || root.actionKind !== "favorite" ? parent.top : undefined
            anchors.bottom: root.poster || root.actionKind !== "favorite" ? undefined : parent.bottom
            anchors.margins: 8
            anchors.rightMargin: posterActions.active ? posterActions.width + 18 : 8
            visible: !!root.ownerSnapshot && !!root.ownerSnapshot.source && root.stacked && !root.soundItem
            opacity: previewHover.hovered || activeFocus || hovered ? 1 : 0
            icon.source: Theme.icon("zoom-in")
            icon.color: Theme.text
            display: AbstractButton.IconOnly
            text: qsTr("View")
            Accessible.name: qsTr("View larger image of %1").arg(root.heading)
            enabled: root.interactable && root.mediaActive && preview.ready && !preview.masked
            background: Rectangle {
                anchors.fill: parent; anchors.margins: -6
                radius: Theme.radius4; color: Theme.surface
                Rectangle { anchors.fill: parent; anchors.margins: 2; radius: parent.radius; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: largerButton.visualFocus }
            }
            Behavior on opacity { NumberAnimation { duration: Theme.motionFast } }
            onClicked: root.openLarger()
        }
    }
    GridLayout {
        id: cardLayout
        transform: liftShift
        anchors.left: parent.left
        anchors.right: parent.right
        // A Library poster sizes its text block to fit, at the bottom over the scrim.
        anchors.top: root.poster && !root.posterFooter ? undefined : parent.top
        anchors.bottom: parent.bottom
        anchors.margins: root.stacked ? 0 : Theme.spacing8
        anchors.rightMargin: root.stacked ? 0 : Theme.spacing12
        columns: root.stacked ? 1 : 3
        columnSpacing: Theme.spacing12
        rowSpacing: Theme.spacing8
        // List mode reserves the art's place here; the art itself is positioned below the layout.
        Item {
            id: artSlot
            visible: !root.poster
            Layout.fillWidth: root.stacked
            Layout.preferredWidth: root.stacked ? -1 : Math.max(112, preview.placeholderMinimumWidth)
            Layout.preferredHeight: root.stacked ? Math.max(1, root.width) * 9 / 16 : 63
            Layout.alignment: Qt.AlignVCenter
        }
        ColumnLayout {
            id: textColumn
            Layout.fillWidth: true
            Layout.fillHeight: root.stacked
            Layout.minimumWidth: 0
            Layout.alignment: root.stacked ? Qt.AlignTop : Qt.AlignVCenter
            spacing: 3
            Layout.leftMargin: root.stacked ? 12 : 0
            Layout.rightMargin: root.stacked ? 12 : 0
            Layout.bottomMargin: root.stacked ? 10 : 0
            // A Browse poster pushes its title down onto the art, just above the footer strip.
            Item { Layout.fillHeight: true; visible: root.posterFooter }
            Button {
                id: titleButton
                objectName: "rowTitle"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                flat: true
                padding: 0
                leftPadding: 0
                rightPadding: 0
                // Size to the title itself; a one-line title must not leave an empty line before the author.
                implicitHeight: Math.max(22, titleText.implicitHeight)
                text: root.heading
                enabled: root.interactable
                Accessible.name: qsTr("Details for %1").arg(root.heading)
                background: Rectangle {
                    color: "transparent"
                    radius: 3
                    border.width: titleButton.visualFocus ? 2 : 0
                    border.color: Theme.accentInk
                }
                ToolTip {
                    id: titleTip
                    visible: titleButton.enabled && titleText.truncated && !root.resultHost.viewerOpen && (titleButton.hovered || titleButton.activeFocus)
                    text: root.heading
                    width: Math.min(440, implicitWidth)
                }
                contentItem: Label {
                    id: titleText
                    objectName: "rowTitleText"
                    text: titleButton.text
                    color: root.switchedOff ? Theme.secondary : Theme.text
                    textFormat: Text.PlainText
                    font.weight: Font.DemiBold
                    // List mode keeps the style's own title font.
                    Binding on font.family { when: root.poster && Theme.displayFamily !== ""; value: Theme.displayFamily }
                    Binding on font.pointSize { when: root.poster; value: Theme.title }
                    wrapMode: (root.twoLineTitle && root.stacked) || root.poster ? Text.Wrap : Text.NoWrap
                    maximumLineCount: (root.twoLineTitle && root.stacked) || root.poster ? 2 : 1
                    lineHeight: root.poster ? .95 : 1
                    elide: Text.ElideRight
                    verticalAlignment: (root.twoLineTitle && root.stacked) || root.poster ? Text.AlignTop : Text.AlignVCenter
                }
                onClicked: { root.resultHost.userInteracted(); root.detailsRequested() }
            }
            Label {
                id: subtitleLabel
                objectName: "rowHeroCategory"
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                text: root.subtitle
                textFormat: Text.PlainText
                color: Theme.secondary
                font.pointSize: Theme.small
                elide: Text.ElideRight
                HoverHandler { id: subtitleHover }
                ToolTip {
                    visible: subtitleHover.hovered && subtitleLabel.truncated
                    text: root.subtitle
                    width: Math.min(440, implicitWidth)
                }
            }
            // Stacked cards keep their badges and actions on the bottom edge, so a grid row lines up.
            Item { Layout.fillHeight: true; visible: root.stacked && !root.posterFooter }
            RowLayout {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                // Reserve the badge line so arriving update/health results cannot resize Library rows.
                // A Library poster's text grows upward over the art instead, so it shows only real badges.
                Layout.minimumHeight: root.actionKind === "switch" && !posterActions.active ? statusLabel.implicitHeight : 0
                // Browse posters put this row in the footer strip under the art.
                Layout.topMargin: root.posterFooter ? 14 : 0
                Layout.preferredHeight: root.posterFooter ? root.posterFooterHeight - 16 : -1
                visible: posterActions.active ? root.updateLabel !== "" || root.healthLabel !== "" || root.status !== ""
                       : root.statsText !== "" || root.updateLabel !== "" || root.healthLabel !== "" || root.status !== "" || root.actionKind === "switch" || (root.stacked && root.actionKind !== "")
                spacing: root.posterFooter ? 6 : Theme.spacing8
                GridLayout {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    objectName: "catalogStatsRow"
                    visible: root.catalogStats.length > 0
                    columns: root.posterFooter ? 1 : Math.max(1, root.catalogStats.length)
                    columnSpacing: 6
                    rowSpacing: 1
                    Accessible.role: Accessible.StaticText
                    Accessible.name: root.statsText
                    ToolTip.visible: statsHover.hovered
                    ToolTip.text: root.statsText
                    HoverHandler { id: statsHover }
                    Repeater {
                        model: root.catalogStats
                        RowLayout {
                            required property var modelData
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            spacing: 3
                            ColorImage {
                                Layout.preferredWidth: 12; Layout.preferredHeight: 12
                                source: Theme.icon(parent.modelData.icon); sourceSize: Qt.size(12, 12)
                                color: Theme.faint
                                Accessible.ignored: true
                            }
                            Label {
                                Layout.fillWidth: true; Layout.minimumWidth: 0
                                text: parent.modelData.text; textFormat: Text.PlainText
                                elide: Text.ElideRight
                                font.pointSize: Theme.small; font.features: { "tnum": 1 }
                                color: Theme.faint
                                Accessible.ignored: true
                            }
                        }
                    }
                }
                StatusPill {
                    id: statusLabel
                    objectName: "rowAuthorStatus"
                    Layout.minimumWidth: 0
                    visible: root.status !== ""
                    text: root.status
                    tone: root.statusTone
                }
                StatusPill { objectName: "modUpdatePill"; visible: root.updateLabel !== ""; text: root.updateLabel; tone: "warning" }
                StatusPill { objectName: "modHealthPill"; visible: root.healthLabel !== ""; text: root.healthLabel; tone: root.healthTone }
                Item { Layout.fillWidth: true; visible: root.stacked && !root.catalogStats.length }
                Loader {
                    active: root.stacked && !posterActions.active
                    visible: active
                    sourceComponent: actionRow
                }
            }
        }
        Loader {
            Layout.alignment: Qt.AlignVCenter
            active: !root.stacked
            visible: active
            sourceComponent: actionRow
        }
    }
    // Library posters keep the switch on the art; declared after the text so Tab still reaches the title first.
    Rectangle {
        transform: liftShift
        anchors.fill: posterActions
        anchors.margins: -2
        radius: height / 2
        color: Qt.alpha(Theme.surface, .72)
        visible: posterActions.active
    }
    Loader {
        id: posterActions
        transform: liftShift
        anchors.top: artBox.top; anchors.right: artBox.right; anchors.margins: 8
        active: root.poster && !root.posterFooter
        visible: active
        sourceComponent: actionRow
    }
    // Rounds the poster's corners: paints the page colour outside a radiusCard outline
    // (clip cannot round, and the software renderer has no mask effect).
    Rectangle {
        transform: liftShift
        visible: root.poster
        anchors.fill: parent
        anchors.margins: -5
        radius: Theme.radiusCard + 5
        color: "transparent"
        border.width: 5
        border.color: Theme.window
    }
    Rectangle {
        objectName: "posterEdge"
        transform: liftShift
        visible: root.poster
        anchors.fill: parent
        radius: Theme.radiusCard
        color: "transparent"
        border.width: root.selected ? 2 : 1
        border.color: root.selected ? Theme.accentRing : cardHover.hovered ? Qt.alpha(Theme.brass, .6) : Theme.rule
        Behavior on border.color { ColorAnimation { duration: Theme.motionFast } }
    }
    Rectangle {
        id: toggleFlashRing
        objectName: "toggleFlashRing"
        transform: liftShift
        anchors.fill: parent
        radius: root.joined ? 0 : Theme.radiusCard
        color: "transparent"
        border.width: 2
        border.color: Theme.brass
        opacity: 0
        visible: opacity > 0.001
        Accessible.ignored: true
    }
    Component {
        id: actionRow
        RowLayout {
            spacing: Theme.spacing4
            Button {
                id: favoriteButton
                objectName: "favoriteButton"
                Layout.preferredWidth: Tokens.controlHeight
                Layout.preferredHeight: Tokens.controlHeight
                Layout.minimumWidth: Tokens.controlHeight
                Layout.minimumHeight: Tokens.controlHeight
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: Tokens.controlHeight
                implicitHeight: Tokens.controlHeight
                padding: 0
                leftPadding: 0
                rightPadding: 0
                visible: root.actionKind === "favorite" && root.actionVisible
                enabled: root.actionEnabled && root.interactable
                checkable: true
                checked: root.actionChecked
                icon.source: Theme.icon(root.actionChecked ? "heart-filled" : "heart")
                icon.color: root.actionChecked ? Theme.accentInk : Theme.secondary
                display: AbstractButton.IconOnly
                text: root.actionText
                Accessible.name: root.actionName
                ToolTip { visible: favoriteButton.hovered || favoriteButton.activeFocus; text: favoriteButton.text }
                onClicked: { root.resultHost.userInteracted(); root.actionRequested(); checked = Qt.binding(() => root.actionChecked) }
            }
            StatusPill { visible: root.installedToggle; text: qsTr("In library"); tone: "brass" }
            Switch {
                objectName: "catalogInstalledSwitch"
                spacing: 0
                implicitHeight: 34
                visible: root.installedToggle
                checked: root.installedOn
                enabled: root.installEnabled && root.interactable
                Accessible.name: (root.installedOn ? qsTr("%1, on in %2") : qsTr("%1, off in %2")).arg(root.heading).arg(root.installedProfile)
                onToggled: {
                    root.resultHost.userInteracted()
                    if (root.interactable && root.installEnabled && checked !== root.installedOn) root.installRequested()
                    checked = Qt.binding(() => root.installedOn)
                }
            }
            Button {
                id: installButton
                objectName: "installButton"
                Layout.preferredHeight: Tokens.controlHeight
                Layout.alignment: Qt.AlignVCenter
                leftPadding: root.posterFooter ? 8 : Tokens.controlPadding
                rightPadding: root.posterFooter ? 8 : Tokens.controlPadding
                spacing: root.posterFooter ? 5 : 6
                visible: root.installVisible && !root.installedToggle
                enabled: root.installEnabled && root.interactable
                highlighted: false
                text: root.installText
                icon.source: Theme.icon(root.installUpdate ? "refresh" : "download")
                icon.color: root.installUpdate ? Theme.warning : Theme.text
                Accessible.name: qsTr("%1 %2").arg(root.installText).arg(root.heading)
                ToolTip { visible: installButton.hovered && !installButton.enabled; text: qsTr("This action isn't available right now") }
                onClicked: { root.resultHost.userInteracted(); root.installRequested() }
            }
            Switch {
                objectName: "modSwitch"
                visible: root.actionKind === "switch" && root.actionVisible
                enabled: root.actionEnabled && root.interactable
                checked: root.actionChecked
                Accessible.name: root.actionName
                onToggled: {
                    root.resultHost.userInteracted()
                    if (root.interactable && root.actionEnabled && checked !== root.actionChecked) root.toggleRequested(checked)
                    checked = Qt.binding(() => root.actionChecked)
                }
            }
        }
    }
}
