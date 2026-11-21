pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Effects
import "HealthText.js" as HealthText
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

Pane {
    id: root
    objectName: "modDetails"
    required property var mod
    required property bool readOnly
    property bool toggleReadOnly: readOnly
    property LibraryModel library: null
    property bool nativeMode: false
    readonly property string profileName: library ? ((library.profiles.find(profile => profile.id === library.viewedProfileId) || {}).name || qsTr("Default")) : qsTr("Default")
    readonly property bool hasModPage: !!(mod.remoteSubmission && mod.remoteSubmission.submissionId)
    property bool fontsExpanded: false
    property bool technicalExpanded: false
    readonly property bool hasFontRecords: !!mod.id && ((mod.fonts || []).length > 0 || (mod.fontOwned || []).length > 0
                                                        || (mod.fontStatus || []).length > 0 || (mod.fontDiagnostics || []).length > 0)
    property string mediaContextKey: ""
    property bool fontFocusReturnAllowed: false
    property FontController fonts: null
    property OptionController options: null
    property RemoteController remote: null
    property Item fontFocusTarget: null
    property string fontFocusMod: ""
    property string fontFocusContext: ""
    property bool fontWasVisible: false
    property int fontSurfaceRevision: 0
    property bool optionsWereVisible: false
    property string optionsFocusContext: ""
    Connections {
        target: root.options
        function onChanged() {
            if (root.optionsWereVisible && !root.options.visible && root.visible
                    && root.optionsFocusContext !== "" && root.optionsFocusContext === root.mediaContextKey
                    && root.options.modId === root.mod.id) {
                const target = filesOptionsButton.visible && filesOptionsButton.enabled ? filesOptionsButton : modActions
                if (target.visible && target.enabled) target.forceActiveFocus()
            }
            if (root.optionsWereVisible && !root.options.visible) root.optionsFocusContext = ""
            root.optionsWereVisible = root.options.visible
        }
    }
    Connections {
        target: root.fonts
        function onChanged() {
            const showing = !!root.fonts && (root.fonts.visible || root.fonts.managerVisible)
            if (showing && !root.fontWasVisible) ++root.fontSurfaceRevision
            if (root.fontWasVisible && !showing && root.fontFocusTarget) {
                const target = root.fontFocusTarget
                const modId = root.fontFocusMod
                const context = root.fontFocusContext
                const revision = root.fontSurfaceRevision
                root.fontFocusTarget = null
                root.fontFocusMod = ""
                root.fontFocusContext = ""
                Qt.callLater(() => {
                    if (!root || !root.visible || !root.enabled || !root.fontFocusReturnAllowed || root.viewerOpen
                            || root.mod.id !== modId || root.mediaContextKey !== context || root.fontSurfaceRevision !== revision
                            || !root.fonts || root.fonts.visible || root.fonts.managerVisible
                            || (root.options && root.options.visible)) return
                    if (target && target.visible && target.enabled) target.forceActiveFocus()
                    else if (fontsDisclosure.visible && fontsDisclosure.enabled) fontsDisclosure.forceActiveFocus()
                    else if (modActions.visible && modActions.enabled) modActions.forceActiveFocus()
                })
            }
            root.fontWasVisible = showing
        }
    }
    property int galleryIndex: 0
    readonly property var images: mod.images || []
    readonly property url currentImage: galleryIndex < images.length ? images[galleryIndex] : ""
    readonly property bool viewerOpen: viewer.opened
    // Spotlight backdrop: only the current, admitted, unmasked foreground artwork. Any source,
    // gallery, context, hide or reveal change drops preview.ready in the same pass.
    readonly property bool spotlightEligible: preview.active && preview.visible && preview.ready && !preview.masked
    // The ordinary Software adaptation cannot run shader effects; it keeps the plain surface.
    readonly property bool spotlightRendered: spotlightEligible && GraphicsInfo.api !== GraphicsInfo.Software
    function syncSpotlight() {
        spotlightFade.stop()
        if (!spotlightEligible) spotlight.opacity = 0
        else if (Tokens.reduceMotion) spotlight.opacity = 1
        else { spotlight.opacity = 0; spotlightFade.start() }
    }
    onSpotlightEligibleChanged: syncSpotlight()
    Connections {
        target: Tokens
        // Reduced motion stops a fade already in progress, not just later ones.
        function onReduceMotionChanged() { if (Tokens.reduceMotion && spotlightFade.running) root.syncSpotlight() }
    }
    NumberAnimation { id: spotlightFade; target: spotlight; property: "opacity"; from: 0; to: 1; duration: Theme.motionNormal; easing.type: Easing.OutCubic }
    signal remoteRequested(var submission)
    signal changelogRequested(var submission)
    signal authorRequested(string authorId)
    signal replacementRequested(string modId, string action)
    signal toggleRequested(string id, bool enabled)
    signal removalRequested(string modId, bool deleteLibrary)
    function metadataLine(mod, hasPage) {
        const parts = []
        const author = Theme.meaningful(mod.author)
        if (root.nativeMode && author && author.toLowerCase() !== "unknown author") parts.push(author)
        if (mod.sizeBytes !== undefined && mod.sizeBytes !== null) parts.push(Theme.bytes(mod.sizeBytes))
        if (mod.variantName) parts.push(mod.variantName)
        if (root.nativeMode && Theme.meaningful(mod.hero)) parts.push(mod.hero)
        if (root.nativeMode && Theme.meaningful(mod.kind)) parts.push(mod.kind)
        return parts.join(" · ")
    }
    function focusDownloads() { if (downloadActions.visible && downloadActions.enabled) downloadActions.forceActiveFocus() }
    function openViewer(invoker) {
        if (!visible || !mod.id) return
        viewer.open({sources: images, token: mediaContextKey, hidden: !!mod.previewHidden,
                     source: String(images[galleryIndex]), startIndex: galleryIndex}, preview, invoker)
    }
    function closeViewer() { viewer.close() }
    onGalleryIndexChanged: if (preview) preview.revealed = false
    property bool ready: false
    property string lastModId: ""
    Component.onCompleted: {
        lastModId = String(mod && mod.id ? mod.id : "")
        ready = true
    }
    onModChanged: {
        const nextId = String(mod && mod.id ? mod.id : "")
        if (ready && nextId !== lastModId) {
            fontsExpanded = false
            technicalExpanded = false
            if (modActionsMenu.visible) modActionsMenu.close()
        }
        lastModId = nextId
        galleryIndex = 0
        closeViewer()
        preview.revealed = false
    }
    onVisibleChanged: if (!visible && modActionsMenu.visible) modActionsMenu.close()
    onMediaContextKeyChanged: if (ready && modActionsMenu.visible) modActionsMenu.close()
    padding: 0
    background: Rectangle { color: Theme.surfaceLow }
    readonly property string technicalSummary: root.mod.fileName || (root.mod.installedVpks || []).join(", ")
    readonly property bool hasTechnicalDetails: !!root.mod.id && (root.nativeMode || technicalSummary !== ""
                                                                   || (root.mod.installOrder !== null && root.mod.installOrder !== undefined)
                                                                   || (root.mod.installedVpks || []).length > 0
                                                                   || (root.mod.missingVpks || []).length > 0)
    readonly property bool canReplace: root.nativeMode && root.hasModPage && !root.mod.linkEligible && !root.mod.linkReason
    readonly property bool replacementEnabled: canReplace && !root.readOnly && !!root.remote && root.remote.replacementAvailable
    readonly property var updateInfo: {
        if (!root.nativeMode || !root.mod.id)
            return { showStrip: false, pillText: "", pillTone: "muted", pillIcon: "", noteText: "" }
        const upd = root.mod.update || ({})
        const status = String(upd.status || "")
        const versionUnknown = upd.versionUnknown === true
        const updReason = String(upd.reason || "")
        const linkReason = String(root.mod.linkReason || "")
        const checking = !!root.library && !!root.library.updatesChecking
        const offline = !!root.library && !!root.library.updatesOffline
        const rawErr = root.library ? String(root.library.updatesError || "") : ""
        const err = rawErr ? Theme.friendlyError(rawErr) : ""
        const hasChecked = !root.library || (root.library.lastUpdateCheck !== undefined && root.library.lastUpdateCheck !== null && root.library.lastUpdateCheck !== "" && Number(root.library.lastUpdateCheck) > 0)
        const onActiveProfile = !root.library || root.library.activeProfileId === root.library.viewedProfileId
        const cardVisible = status === "available" || versionUnknown

        if (root.mod.linkEligible || !root.hasModPage) {
            if (root.mod.linkEligible) {
                return {
                    showStrip: true,
                    pillText: qsTr("Not linked"),
                    pillTone: "muted",
                    pillIcon: "",
                    noteText: qsTr("Link to GameBanana to check for updates.")
                }
            }
            return {
                showStrip: true,
                pillText: qsTr("Unlinked"),
                pillTone: "muted",
                pillIcon: "",
                noteText: linkReason ? qsTr("GameBanana link unavailable: %1").arg(Theme.friendlyError(linkReason))
                                     : qsTr("This mod isn't linked to a GameBanana page.")
            }
        }
        if (checking) {
            return {
                showStrip: true,
                pillText: qsTr("Checking for updates…"),
                pillTone: "accent",
                pillIcon: "refresh",
                noteText: ""
            }
        }
        if (err !== "") {
            return {
                showStrip: true,
                pillText: qsTr("Update check failed"),
                pillTone: "danger",
                pillIcon: "alert",
                noteText: err
            }
        }
        if (offline) {
            return {
                showStrip: true,
                pillText: qsTr("Offline"),
                pillTone: "warning",
                pillIcon: "alert",
                noteText: qsTr("Couldn't reach GameBanana to check for updates.")
            }
        }
        if (cardVisible) {
            return {
                showStrip: !!linkReason && !root.mod.linkEligible,
                pillText: "",
                pillTone: "muted",
                pillIcon: "",
                noteText: !root.mod.linkEligible && linkReason ? Theme.friendlyError(linkReason) : ""
            }
        }
        if (status === "unsure") {
            return {
                showStrip: true,
                pillText: qsTr("Version couldn't be confirmed"),
                pillTone: "warning",
                pillIcon: "alert",
                noteText: [updReason ? Theme.friendlyError(updReason) : "",
                           linkReason && linkReason !== updReason ? Theme.friendlyError(linkReason) : ""]
                    .filter(Boolean).join("\n") || qsTr("Updates couldn't be confirmed automatically.")
            }
        }
        if (status === "current" && hasChecked && String(upd.modId || "") === String(root.mod.id)) {
            return {
                showStrip: true,
                pillText: qsTr("No update found at last check"),
                pillTone: "muted",
                pillIcon: "check",
                noteText: linkReason
                    ? Theme.friendlyError(linkReason) + "\n" + qsTr("Saved result. Use Check for updates at the top of Library to refresh it.")
                    : qsTr("Based on saved update data. Check again for the latest files.")
            }
        }
        return {
            showStrip: true,
            pillText: qsTr("Not checked yet"),
            pillTone: "muted",
            pillIcon: "",
            noteText: !root.mod.linkEligible && linkReason ? Theme.friendlyError(linkReason)
                : !onActiveProfile ? qsTr("Switch to the active profile to check updates.")
                : qsTr("Run Check for updates to see if a newer file is available.")
        }
    }
    readonly property string modState: !mod.id ? ""
        : mod.available === false ? "unavailable"
        : !nativeMode ? (mod.enabled ? "enabled" : "disabled")
        : mod.pendingStatus === "selectionRequired" ? "choose"
        : mod.pendingStatus === "unknown" || !mod.pendingStatus ? "check"
        : mod.pendingApply || mod.pendingStatus === "pending" ? "pending"
        : mod.appliedEnabled ? "applied" : mod.enabled ? "saved" : "off"
    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            width: parent.width
            spacing: Theme.spacing16
            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Theme.spacing8
                // Decorative backdrop behind the preview and title. It overflows this spacer so the
                // column layout, actions and focus order stay exactly as they were.
                Item {
                    id: spotlight
                    objectName: "detailSpotlight"
                    readonly property bool fading: spotlightFade.running
                    width: parent.width
                    // Bound the decorative target independently of imported title length.
                    height: Math.min(384, Math.max(0, titleBlock.y + titleLabel.y + titleLabel.height + Theme.spacing8))
                    visible: root.spotlightEligible
                    opacity: 0
                    enabled: false
                    Accessible.ignored: true
                    Loader {
                        objectName: "detailSpotlightEffect"
                        anchors.fill: parent
                        active: root.spotlightRendered
                        sourceComponent: Item {
                            MultiEffect {
                                anchors.fill: parent
                                source: preview.artworkItem
                                autoPaddingEnabled: false
                                blurEnabled: true
                                blur: 1
                                blurMax: 48
                                saturation: .45
                                brightness: Tokens.dark ? -.32 : .22
                                opacity: Tokens.dark ? .8 : .6
                                maskEnabled: true
                                maskSource: spotlightMask
                                maskThresholdMin: .5
                                maskSpreadAtMin: 1
                            }
                            // Alpha fade toward the bottom, so the backdrop blends into whatever surface hosts it.
                            Item {
                                id: spotlightMask
                                anchors.fill: parent
                                visible: false
                                layer.enabled: true
                                Rectangle {
                                    anchors.fill: parent
                                    gradient: Gradient {
                                        GradientStop { position: 0; color: Qt.rgba(1, 1, 1, 1) }
                                        GradientStop { position: .5; color: Qt.rgba(1, 1, 1, .85) }
                                        GradientStop { position: 1; color: Qt.rgba(1, 1, 1, 0) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ModPreview {
                id: preview
                objectName: "detailPreview"
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                Layout.preferredHeight: Math.round(width * 9 / 16)
                radius: Theme.radius
                source: root.currentImage
                hidden: root.mod.previewHidden || false
                contextKey: root.mediaContextKey
                description: qsTr("Preview of %1").arg(root.mod.title || "")
                placeholderTitle: root.mod.title || ""
                allowReveal: true
                active: root.visible
                TapHandler { enabled: preview.ready && !root.viewerOpen; onTapped: root.openViewer(largerButton) }
                HoverHandler { id: previewHover; cursorShape: preview.ready ? Qt.PointingHandCursor : Qt.ArrowCursor }
                Row {
                    anchors.left: parent.left
                    anchors.bottom: parent.bottom
                    anchors.margins: 8
                    spacing: 4
                    visible: root.images.length > 1
                    ToolButton {
                        objectName: "previousPreview"
                        implicitWidth: 28
                        implicitHeight: 28
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon("chevron-left")
                        icon.color: "white"
                        text: qsTr("Previous")
                        enabled: root.galleryIndex > 0
                        opacity: enabled ? 1 : 0.4
                        background: Rectangle { radius: 14; color: Qt.rgba(0, 0, 0, parent.hovered ? 0.7 : 0.5) }
                        onClicked: --root.galleryIndex
                    }
                    Rectangle {
                        height: 28
                        width: counter.implicitWidth + 16
                        radius: 14
                        color: Qt.rgba(0, 0, 0, 0.5)
                        Label {
                            id: counter
                            anchors.centerIn: parent
                            text: qsTr("%1 / %2").arg(root.galleryIndex + 1).arg(root.images.length)
                            font.pointSize: Theme.small
                            color: "white"
                        }
                    }
                    ToolButton {
                        objectName: "nextPreview"
                        implicitWidth: 28
                        implicitHeight: 28
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon("chevron-right")
                        icon.color: "white"
                        text: qsTr("Next")
                        enabled: root.galleryIndex + 1 < root.images.length
                        opacity: enabled ? 1 : 0.4
                        background: Rectangle { radius: 14; color: Qt.rgba(0, 0, 0, parent.hovered ? 0.7 : 0.5) }
                        onClicked: ++root.galleryIndex
                    }
                }
                ToolButton {
                    id: largerButton
                    objectName: "enlargePreview"
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    anchors.margins: 8
                    implicitWidth: 28
                    implicitHeight: 28
                    visible: root.images.length > 0
                    enabled: preview.ready
                    display: AbstractButton.IconOnly
                    icon.source: Theme.icon("zoom-in")
                    icon.color: "white"
                    text: qsTr("View larger")
                    Accessible.name: text
                    ToolTip { visible: largerButton.hovered; text: largerButton.text }
                    background: Rectangle { radius: 14; color: Qt.rgba(0, 0, 0, largerButton.hovered ? 0.7 : 0.5) }
                    onClicked: root.openViewer(largerButton)
                }
            }
            ColumnLayout {
                id: titleBlock
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                spacing: Theme.spacing4
                Label {
                    id: titleLabel
                    objectName: "detailTitle"
                    Layout.fillWidth: true
                    text: root.mod.title || qsTr("Select a mod")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.family: Theme.displayFamily
                    font.pointSize: Theme.heading
                    font.weight: Font.DemiBold
                }
                Label {
                    objectName: "detailAuthor"
                    Layout.fillWidth: true
                    visible: !root.nativeMode && text !== ""
                    text: Theme.meaningful(root.mod.author) && root.mod.author.toLowerCase() !== "unknown author" ? qsTr("by %1").arg(root.mod.author) : ""
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.secondary
                }
                Flow {
                    visible: !root.nativeMode
                    Layout.fillWidth: true
                    Layout.topMargin: Theme.spacing4
                    spacing: 6
                    StatusPill { objectName: "detailHero"; visible: !!Theme.meaningful(root.mod.hero); text: root.mod.hero || "" }
                    StatusPill { objectName: "detailKind"; visible: !!Theme.meaningful(root.mod.kind); text: root.mod.kind || "" }
                }
            }
            Label {
                objectName: "detailMetadata"
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                visible: !!root.mod.id && text.length > 0
                text: root.metadataLine(root.mod, root.hasModPage)
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            Rectangle {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                visible: !!root.mod.id
                implicitHeight: stateColumn.implicitHeight + Theme.spacing24
                radius: Theme.radiusCard
                color: Theme.surface
                border.color: Theme.rule
                ColumnLayout {
                    id: stateColumn
                    anchors.fill: parent
                    anchors.margins: Theme.spacing12
                    spacing: Theme.spacing8
                    RowLayout {
                        Layout.fillWidth: true
                        Label {
                            Layout.fillWidth: true
                            id: stateLabel
                            objectName: "profileStateLabel"
                            Layout.minimumWidth: 0
                            elide: Text.ElideRight
                            textFormat: Text.PlainText
                            text: root.mod.enabled ? qsTr("On in %1").arg(root.profileName) : qsTr("Off in %1").arg(root.profileName)
                            font.weight: Font.Medium
                        }
                        Switch {
                            objectName: "modSwitch"
                            Layout.minimumWidth: implicitWidth
                            Layout.maximumWidth: implicitWidth
                            checked: root.mod.enabled || false
                            enabled: !root.toggleReadOnly && root.mod.id !== undefined && (!root.nativeMode || root.mod.selectionReady || root.mod.enabled)
                            Accessible.name: stateLabel.text
                            onToggled: { root.toggleRequested(root.mod.id, checked); checked = Qt.binding(() => root.mod.enabled || false) }
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        // The switch already says on/off; only show states it doesn't.
                        visible: ["unavailable", "choose", "check"].includes(root.modState)
                                 || (!root.nativeMode && !!root.mod.status && !["installed", "enabled", "disabled", "active"].includes(root.mod.status.toLowerCase()))
                        StatusPill {
                            objectName: "detailStatus"
                            text: root.modState === "unavailable" ? qsTr("Unavailable")
                                : root.modState === "choose" ? qsTr("Choose files")
                                : root.modState === "check" ? qsTr("Needs check")
                                : root.modState === "pending" ? (root.mod.enabled ? qsTr("Ready for next game") : qsTr("Off for next game"))
                                : root.modState === "applied" ? qsTr("Active in game")
                                : root.modState === "saved" ? qsTr("Ready for next game")
                                : root.modState === "enabled" ? (root.mod.status || qsTr("Enabled"))
                                : root.modState === "disabled" ? (root.mod.status || qsTr("Disabled")) : qsTr("Off")
                            tone: root.modState === "unavailable" ? "danger"
                                : root.modState === "choose" || root.modState === "check" ? "warning"
                                : root.modState === "pending" ? "accent"
                                : root.modState === "applied" || root.modState === "enabled" ? "success" : "muted"
                        }
                        Item { Layout.fillWidth: true }
                    }
                    Label {
                        objectName: "appliedModStatus"
                        Layout.fillWidth: true
                        visible: text !== ""
                        text: !root.nativeMode || !root.mod.id ? ""
                            : root.modState === "choose" ? qsTr("Pick which files to use before enabling this mod.")
                            : root.modState === "check" ? qsTr("Review your game setup before playing.")
                            : ""
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.secondary
                        font.pointSize: Theme.small
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.nativeMode && !!root.mod.pendingReason && root.modState !== "pending"
                        text: Theme.friendlyError(root.mod.pendingReason || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.secondary
                        font.pointSize: Theme.small
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.modState === "unavailable" && !!root.mod.unavailableReason
                        text: Theme.friendlyError(root.mod.unavailableReason || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.secondary
                        font.pointSize: Theme.small
                    }
                }
            }
            Button {
                objectName: "modAuthorButton"
                visible: !!root.mod.authorId
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                text: qsTr("More by author")
                flat: true
                icon.source: Theme.icon("user")
                onClicked: root.authorRequested(root.mod.authorId)
            }
            Callout {
                objectName: "modUpdateCard"
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                Layout.fillWidth: true
                tone: "warning"
                visible: !!root.mod.update && (root.mod.update.status === "available" || root.mod.update.versionUnknown === true)
                title: root.mod.update && root.mod.update.versionUnknown === true ? qsTr("Version unknown") : qsTr("Update available")
                text: root.mod.update && root.mod.update.versionUnknown === true
                    ? qsTr("The installed version could not be identified. Update to the latest file.")
                    : qsTr("A newer file is available for this mod.")
                Button {
                    objectName: "modUpdateButton"
                    text: qsTr("Update…")
                    highlighted: true
                    enabled: !root.readOnly && !!root.remote && root.remote.replacementAvailable
                    onClicked: root.replacementRequested(root.mod.id, "update")
                }
                Button {
                    text: qsTr("See changes")
                    flat: true
                    visible: root.hasModPage
                    onClicked: root.changelogRequested(root.mod.remoteSubmission)
                }
            }
            Callout {
                tone: "warning"
                objectName: "modHealthCard"
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                readonly property var health: root.mod.health || ({})
                visible: HealthText.label(health.status) !== ""
                title: ["fixed", "partlyFixed"].includes((root.mod.health || {}).status) ? qsTr("Fixed for this patch") : (root.mod.health || {}).status === "needsUpdate" ? qsTr("Waiting for the author") : qsTr("Fix available for this patch")
                ComboBox {
                    objectName: "modFixChoice"
                    Layout.fillWidth: true
                    visible: !!root.mod.health && ["fixable", "partlyFixable", "fixed", "partlyFixed", "originalFiles"].includes(root.mod.health.status)
                    model: [qsTr("Fix for this patch"), qsTr("Use original files")]
                    currentIndex: root.mod.fixEnabled === false ? 1 : 0
                    enabled: !root.readOnly && !!root.library
                    Accessible.name: qsTr("Files to use for this patch")
                    onActivated: root.library.setFixEnabled(root.mod.id, currentIndex === 0)
                }
            }
            ColumnLayout {
                id: modUpdateStatusStrip
                objectName: "modUpdateStatusStrip"
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                spacing: Theme.spacing4
                visible: root.nativeMode && !!root.mod.id && root.updateInfo.showStrip
                StatusPill {
                    objectName: "modUpdateStatusPill"
                    visible: root.updateInfo.pillText !== ""
                    text: root.updateInfo.pillText
                    tone: root.updateInfo.pillTone
                    iconName: root.updateInfo.pillIcon
                    Layout.maximumWidth: parent.width
                }
                Label {
                    objectName: "modUpdateStatusNote"
                    Layout.fillWidth: true
                    visible: root.updateInfo.noteText !== ""
                    text: root.updateInfo.noteText
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.pointSize: Theme.small
                    color: Theme.secondary
                }
            }
            Flow {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                spacing: Theme.spacing4
                // From the conditions, not the children's visible (which follows this item's own visibility).
                visible: !!root.mod.id && (root.nativeMode || root.hasModPage || root.hasTechnicalDetails)
                Button {
                    id: modCheckUpdatesButton
                    objectName: "modCheckUpdatesButton"
                    visible: root.canReplace
                    enabled: root.replacementEnabled && !!root.library && !root.library.updatesChecking
                             && root.library.activeProfileId === root.library.viewedProfileId
                    text: qsTr("Check for updates")
                    icon.source: Theme.icon("refresh")
                    onClicked: root.library.checkForUpdates()
                }
                ToolButton {
                    id: modActions
                    objectName: "modActionsButton"
                    visible: root.nativeMode || root.hasTechnicalDetails
                    text: qsTr("More mod actions")
                    icon.source: Theme.icon("more")
                    display: AbstractButton.IconOnly
                    ToolTip.visible: hovered || visualFocus
                    ToolTip.text: qsTr("More actions")
                    onClicked: modActionsMenu.popup(modActions, 0, modActions.height + 4)
                    Menu {
                        id: modActionsMenu
                        objectName: "modActionsMenu"
                        MenuItem {
                            objectName: "reinstallModButton"
                            text: qsTr("Reinstall…")
                            visible: root.canReplace
                            enabled: root.replacementEnabled
                            onTriggered: root.replacementRequested(root.mod.id, "reinstall")
                        }
                        MenuItem {
                            objectName: "linkButton"
                            text: qsTr("Link to GameBanana for updates…")
                            visible: root.nativeMode && !!root.mod.linkEligible
                            enabled: !root.readOnly && !!root.remote && root.remote.replacementAvailable
                            onTriggered: root.replacementRequested(root.mod.id, "link")
                        }
                        MenuItem {
                            objectName: "technicalDetailsButton"
                            text: root.technicalExpanded ? qsTr("Hide technical details") : qsTr("Technical details")
                            visible: root.hasTechnicalDetails
                            onTriggered: root.technicalExpanded = !root.technicalExpanded
                        }
                        MenuSeparator { visible: root.nativeMode }
                        MenuItem {
                            objectName: "removeModButton"
                            text: qsTr("Remove from profile…")
                            visible: root.nativeMode
                            enabled: !root.readOnly && !!root.library && root.library.canBulkOperate
                            onTriggered: root.removalRequested(root.mod.id, false)
                        }
                        MenuItem {
                            objectName: "deleteModButton"
                            text: qsTr("Delete from library…")
                            visible: root.nativeMode
                            enabled: !root.readOnly && !!root.library && root.library.canDeleteMods
                            onTriggered: root.removalRequested(root.mod.id, true)
                        }
                    }
                }
                Button {
                    id: filesOptionsButton
                    objectName: "filesOptionsButton"
                    visible: root.nativeMode
                    enabled: !!root.options && root.options.available && !!root.mod.id
                    text: qsTr("Files and options…")
                    icon.source: Theme.icon("sliders")
                    onClicked: {
                        root.optionsFocusContext = root.mediaContextKey
                        if (!root.options.open(root.mod.id)) root.optionsFocusContext = ""
                    }
                }
                Button {
                    id: providerButton
                    objectName: "modPageButton"
                    visible: root.hasModPage
                    text: qsTr("Mod page")
                    flat: true
                    icon.source: Theme.icon("external")
                    Accessible.description: qsTr("Open this mod's GameBanana page inside the app")
                    onClicked: root.remoteRequested(root.mod.remoteSubmission)
                }

            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing16
                Layout.rightMargin: Theme.spacing16
                spacing: Theme.spacing8
                visible: !!root.mod.id && (root.technicalExpanded || root.hasFontRecords)
                Rectangle {
                    id: technicalDetailsPanel
                    objectName: "technicalDetailsPanel"
                    Layout.fillWidth: true
                    visible: root.technicalExpanded
                    implicitHeight: technicalLayout.implicitHeight + Theme.spacing24
                    radius: Theme.radiusCard
                    color: Theme.surface
                    border.color: Theme.rule
                    ColumnLayout {
                        id: technicalLayout
                        anchors.fill: parent
                        anchors.margins: Theme.spacing12
                        spacing: Theme.spacing8
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacing8
                            Label {
                                Layout.fillWidth: true
                                text: qsTr("Technical details")
                                font.pointSize: Theme.small
                                font.weight: Font.DemiBold
                                color: Theme.secondary
                            }
                            Button {
                                objectName: "hideTechnicalDetailsButton"
                                flat: true
                                text: qsTr("Hide")
                                onClicked: root.technicalExpanded = false
                            }
                        }
                        Label {
                            objectName: "detailFileName"
                            Layout.fillWidth: true
                            visible: text !== ""
                            text: root.technicalSummary
                            textFormat: Text.PlainText
                            wrapMode: Text.WrapAnywhere
                            color: Theme.secondary
                            font.pointSize: Theme.small
                        }
                        Label {
                            objectName: "detailModId"
                            Layout.fillWidth: true
                            visible: root.nativeMode && !!root.mod.id
                            text: root.mod.id ? qsTr("Mod ID: %1").arg(root.mod.id) : ""
                            textFormat: Text.PlainText
                            wrapMode: Text.WrapAnywhere
                            color: Theme.secondary
                            font.pointSize: Theme.small
                        }
                        Label {
                            objectName: "detailInstallOrder"
                            Layout.fillWidth: true
                            visible: root.mod.installOrder !== null && root.mod.installOrder !== undefined
                            text: visible ? qsTr("Install order: %1").arg(root.mod.installOrder) : ""
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            font.pointSize: Theme.small
                            color: Theme.secondary
                        }
                        Label {
                            objectName: "detailFiles"
                            Layout.fillWidth: true
                            readonly property var installed: root.mod.installedVpks || []
                            readonly property var missing: root.mod.missingVpks || []
                            readonly property string filesText: {
                                const sections = []
                                if (installed.length > 0) sections.push(qsTr("Installed packages:\n%1").arg(installed.join("\n")))
                                if (missing.length > 0) sections.push(qsTr("Missing packages:\n%1").arg(missing.join("\n")))
                                return sections.join("\n\n")
                            }
                            visible: filesText !== ""
                            text: filesText
                            textFormat: Text.PlainText
                            wrapMode: Text.WrapAnywhere
                            font.pointSize: Theme.small
                            color: Theme.secondary
                        }
                    }
                }
            Button {
                id: fontsDisclosure
                Layout.fillWidth: true
                visible: root.hasFontRecords
                text: root.fontsExpanded ? qsTr("Hide fonts") : qsTr("Show fonts")
                Accessible.name: text
                onClicked: root.fontsExpanded = !root.fontsExpanded
            }
            Label {
                Layout.fillWidth: true
                visible: root.hasFontRecords && (root.mod.fontDiagnostics || []).length > 0
                text: qsTr("%1 unavailable font sources are recorded as history. They cannot be installed.").arg(Number((root.mod.fontDiagnostics || []).length).toLocaleString(Qt.locale(), 'f', 0))
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: root.nativeMode && root.hasFontRecords && root.fontsExpanded
                text: qsTr("Shared fonts remain installed when switching profiles or disabling mods.")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            Label {
                Layout.fillWidth: true
                visible: root.fontsExpanded && (root.mod.fontStatus || []).length > 0
                text: qsTr("Bundled fonts: %1 saved assets; %2 installed or borrowed references. Review the list for each file’s status.")
                    .arg(Number((root.mod.fonts || []).length).toLocaleString(Qt.locale(), 'f', 0))
                    .arg(Number((root.mod.fontOwned || []).length).toLocaleString(Qt.locale(), 'f', 0))
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Button {
                id: installFonts
                Layout.fillWidth: true
                visible: root.fontsExpanded && (root.mod.fonts || []).length > 0
                text: qsTr("Install fonts…")
                enabled: root.fonts !== null && root.fonts.available
                onClicked: {
                    root.fontFocusTarget = installFonts
                    root.fontFocusMod = root.mod.id
                    root.fontFocusContext = root.mediaContextKey
                    if (!root.fonts.open(root.mod.id, "install")) { root.fontFocusTarget = null; root.fontFocusMod = ""; root.fontFocusContext = "" }
                }
            }
            ListView {
                id: fontStatus
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(180, count * 60)
                visible: root.fontsExpanded && count > 0
                clip: true
                reuseItems: true
                activeFocusOnTab: true
                model: root.mod.fontStatus || []
                ScrollBar.vertical: ScrollBar {}
                delegate: Label {
                    required property var modelData
                    width: fontStatus.width
                    padding: 4
                    text: qsTr("%1\n%2").arg(modelData.targetName).arg(modelData.status === "installed" ? qsTr("Installed") : modelData.status === "borrowed" ? qsTr("Borrowed existing file") : qsTr("Stashed in library"))
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
            }
            Button {
                id: removeFonts
                Layout.fillWidth: true
                visible: root.fontsExpanded && (root.mod.fontOwned || []).length > 0
                text: qsTr("Remove installed fonts…")
                enabled: root.fonts !== null && root.fonts.available
                onClicked: {
                    root.fontFocusTarget = removeFonts
                    root.fontFocusMod = root.mod.id
                    root.fontFocusContext = root.mediaContextKey
                    if (!root.fonts.open(root.mod.id, "remove")) { root.fontFocusTarget = null; root.fontFocusMod = ""; root.fontFocusContext = "" }
                }
            }
            }
            Item { Layout.preferredHeight: Theme.spacing16 }
        }
    }
    ViewerSession {
        id: viewer
        valid: !!snapshot && root.visible && !!root.mod.id && root.images.length > 0
               && snapshot.token === root.mediaContextKey && snapshot.hidden === !!root.mod.previewHidden
               && viewer.sameSources(snapshot.sources, root.images)
        title: root.mod.title || qsTr("Mod preview")
        onFocusRequested: (state, owner, target, index) => {
            if (!root.visible) return
            if (target && target.visible && target.enabled) target.forceActiveFocus()
            else if (largerButton.visible && largerButton.enabled) largerButton.forceActiveFocus()
            else root.forceActiveFocus()
        }
        onIndexSelected: index => root.galleryIndex = index
        onImageRevealed: (index, source, contextKey) => {
            if (contextKey === root.mediaContextKey && index === root.galleryIndex
                    && String(root.images[index] || "") === source) preview.revealed = true
        }
    }
}
