pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

ToolBar {
    id: root
    required property LibraryModel library
    signal openSourceRequested()
    signal importRequested()
    signal restoreRequested()
    signal profilesRequested()
    signal applyRequested()
    signal launchSettingsRequested()
    signal launchDetailsRequested()
    signal downloadsRequested()
    readonly property bool popupOpen: more.visible || launchMenu.visible || profiles.popup.visible
    readonly property bool differentLaunchProfile: !library.demoMode && !!library.activeProfileId && library.activeProfileId !== library.viewedProfileId
    readonly property string launchProgress: library.launch.gameStatus.state === "running" || library.launch.phase === "running" ? qsTr("Running")
        : ["openingSteam", "waitingForGame"].includes(library.launch.phase) ? qsTr("Starting…")
        : ["reviewing", "ready", "preparing", "verifyingPrepared"].includes(library.launch.phase) ? qsTr("Preparing…") : ""
    readonly property string launchTarget: library.launch.activeProfileName || library.activeProfileId || qsTr("active profile")
    readonly property bool playPendingChanges: root.library.nativeMode
        && !root.library.demoMode
        && !root.library.readOnly
        && !root.library.loading
        && !root.library.busy
        && !root.library.outcomeUnknown
        && !root.library.recoveryRequired
        && !root.library.externalGameinfoChange
        && root.library.error === ""
        && !root.differentLaunchProfile
        && root.library.pendingStatus === "pending"
        && root.library.pendingChanges
    function profileInitial(name) {
        return TextUtils.initial(String(name || ""))
    }
    property bool focusReturnAllowed: true
    property bool downloadsAllowed: true
    property bool suppressMoreFocus: false
    property bool suppressLaunchFocus: false
    property bool optionsWereVisible: false
    property bool fontsWereVisible: false
    property bool fontReturnToMore: false
    property bool performanceVisible: false
    property string lastLaunchPhase: root.library.launch.phase
    function resetPlayIgnition() {
        ignitionGlyphAnim.stop()
        playGlyphIcon.opacity = 1
        playGlyphIcon.rotation = 0
        playGlyphIcon.scale = 1
        playSunburstIcon.opacity = 0
        playSunburstIcon.rotation = -180
        playSunburstIcon.scale = 0.65
    }
    function triggerPlayIgnition() {
        if (Tokens.reduceMotion || Tokens.fast <= 0 || !launchButton.visible || !launchButton.Window.window
                || !launchButton.Window.window.visible || launchButton.Window.visibility === Window.Minimized) {
            resetPlayIgnition()
            playPlaque.stopIgnition()
            return
        }
        playPlaque.ignite()
        ignitionGlyphAnim.restart()
    }
    Connections {
        target: root.library.launch
        function onChanged() {
            const prev = root.lastLaunchPhase
            const next = root.library.launch.phase
            root.lastLaunchPhase = next
            const idlePhases = ["idle", "completed", "cancelled", "failed", "timedOut", "resolved", ""]
            const startPhases = ["reviewing", "ready", "preparing", "verifyingPrepared", "openingSteam", "waitingForGame"]
            if (prev !== next && idlePhases.includes(prev) && startPhases.includes(next))
                root.triggerPlayIgnition()
        }
    }
    Connections {
        target: Tokens
        function onReduceMotionChanged() {
            if (Tokens.reduceMotion) {
                root.resetPlayIgnition()
                playPlaque.stopIgnition()
            }
        }
    }
    function focusLaunch() {
        if (!root.visible || !root.focusReturnAllowed) return
        if (launchButton.visible && launchButton.enabled) launchButton.forceActiveFocus()
        else if (launchMenuButton.visible && launchMenuButton.enabled) launchMenuButton.forceActiveFocus()
        else focusMore()
    }
    function focusProfiles() {
        if (!root.visible || !root.focusReturnAllowed) return
        if (profiles.visible && profiles.enabled) {
            profiles.focus = false
            profiles.forceActiveFocus(Qt.TabFocusReason)
        }
        else focusMore()
    }
    function openMoreForScreenshot() { more.popup(moreButton, moreButton.width - more.width, moreButton.height + 4) }
    function focusMore() { if (root.visible && root.focusReturnAllowed && moreButton.visible && moreButton.enabled) moreButton.forceActiveFocus() }
    Connections {
        target: root.library.options
        function onChanged() {
            const showing = root.library.options.historyVisible || root.library.options.orderVisible
            if (root.optionsWereVisible && !showing && !root.library.options.visible) Qt.callLater(root.focusMore)
            root.optionsWereVisible = showing
        }
    }
    Connections {
        target: root.library.fonts
        function onChanged() {
            const showing = root.library.fonts.managerVisible || root.library.fonts.visible
            if (root.fontsWereVisible && !showing && root.fontReturnToMore) {
                root.fontReturnToMore = false
                Qt.callLater(root.focusMore)
            }
            root.fontsWereVisible = showing
        }
    }
    leftPadding: Theme.spacing16
    rightPadding: Theme.spacing16
    topPadding: Theme.spacing8
    bottomPadding: Theme.spacing8
    implicitHeight: Math.max(56, headerLayout.implicitHeight + topPadding + bottomPadding)
    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0; color: "transparent" }
            GradientStop { position: 0.5; color: Qt.alpha(Theme.brass, 0.45) }
            GradientStop { position: 1; color: "transparent" }
        }
    }
    FontMetrics { id: wordmarkMetrics; font.pointSize: Theme.wordmark }
    RowLayout {
        id: headerLayout
        anchors.fill: parent
        spacing: Theme.spacing8
        Item {
            id: logoMark
            objectName: "headerLogoMark"
            Layout.preferredWidth: 32
            Layout.preferredHeight: 32
            Accessible.ignored: true
            Rectangle {
                objectName: "headerLogoHaloOuter"
                anchors.centerIn: parent
                width: 40
                height: 40
                radius: 20
                color: Qt.alpha(Theme.live, Tokens.dark ? 0.14 : 0.11)
                Accessible.ignored: true
            }
            Rectangle {
                objectName: "headerLogoHaloInner"
                anchors.centerIn: parent
                width: 30
                height: 30
                radius: 15
                color: Qt.alpha(Theme.live, Tokens.dark ? 0.22 : 0.16)
                Accessible.ignored: true
            }
            Image {
                anchors.fill: parent
                source: "qrc:/modlock.svg"
                sourceSize: Qt.size(32, 32)
                Accessible.ignored: true
            }
        }
        Label {
            visible: root.width >= 900
            Layout.rightMargin: Theme.spacing12
            text: qsTr("MODLOCK")
            font.family: Theme.displayFamily
            font.pointSize: Theme.wordmark
            font.weight: Font.DemiBold
            font.letterSpacing: 0.16 * wordmarkMetrics.font.pixelSize
        }
        ComboBox {
            id: profiles
            objectName: "profileChooser"
            visible: count > 0
            leftPadding: 6
            rightPadding: 12 + (indicator ? indicator.width + spacing : 0)
            Layout.preferredWidth: Math.min(Math.max(150, Math.ceil(profileMetrics.advanceWidth) + 24 + Theme.spacing8 + leftPadding + rightPadding + 4), root.width * 0.28)
            Layout.maximumWidth: root.width * 0.28
            Layout.minimumWidth: 100
            model: root.library.profiles
            textRole: "name"
            valueRole: "id"
            enabled: root.library.canBrowseProfiles && count > 0
            currentIndex: root.library.profiles.findIndex(profile => profile.id === root.library.viewedProfileId)
            Accessible.name: qsTr("Viewed profile: %1").arg(displayText)
            Accessible.description: qsTr("Choose a profile to view or edit. Use Manage profiles to play with it.")
            onActivated: root.library.viewProfile(currentValue)
            Keys.onPressed: event => {
                if (!profiles.popup.visible) return
                if (event.key === Qt.Key_Escape) {
                    profiles.popup.close()
                    profiles.forceActiveFocus(Qt.ShortcutFocusReason)
                    event.accepted = true
                } else if (event.key === Qt.Key_Backtab || (event.key === Qt.Key_Tab && (event.modifiers & Qt.ShiftModifier))) {
                    const prev = profiles.nextItemInFocusChain(false)
                    profiles.popup.close()
                    if (prev) prev.forceActiveFocus(Qt.BacktabFocusReason)
                    event.accepted = true
                } else if (event.key === Qt.Key_Tab) {
                    if (manageProfilesItem.enabled) {
                        manageProfilesItem.forceActiveFocus(Qt.TabFocusReason)
                    } else {
                        const next = profiles.nextItemInFocusChain(true)
                        profiles.popup.close()
                        if (next) next.forceActiveFocus(Qt.TabFocusReason)
                    }
                    event.accepted = true
                } else if (event.key === Qt.Key_Down && manageProfilesItem.enabled && (profiles.highlightedIndex >= profiles.count - 1 || profiles.count === 0)) {
                    manageProfilesItem.forceActiveFocus(Qt.TabFocusReason)
                    event.accepted = true
                }
            }
            contentItem: RowLayout {
                spacing: Theme.spacing8
                Item {
                    objectName: "profileInitialBadge"
                    Layout.preferredWidth: 24
                    Layout.preferredHeight: 24
                    Layout.alignment: Qt.AlignVCenter
                    Accessible.ignored: true
                    ColorImage {
                        anchors.fill: parent
                        source: Theme.icon("sunburst-rays")
                        sourceSize: Qt.size(24, 24)
                        color: Qt.alpha(Theme.brass, profiles.enabled ? 0.52 : 0.25)
                        Accessible.ignored: true
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 16
                        height: 16
                        radius: 8
                        color: profiles.enabled ? Theme.brass : Theme.surfaceHigh
                        border.width: 1
                        border.color: profiles.enabled ? Qt.alpha(Theme.window, 0.25) : Theme.rule
                        Accessible.ignored: true
                        Label {
                            objectName: "profileInitialText"
                            anchors.fill: parent
                            anchors.margins: 1
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            text: root.profileInitial(profiles.displayText)
                            textFormat: Text.PlainText
                            font.family: Theme.displayFamily
                            font.pointSize: Theme.small
                            font.weight: Font.Bold
                            fontSizeMode: Text.Fit
                            minimumPixelSize: 8
                            color: !profiles.enabled ? Theme.faint : (Tokens.relativeLuminance(Theme.brass) > 0.35 ? "#14101c" : "#ffffff")
                            Accessible.ignored: true
                        }
                    }
                }
                Label {
                    objectName: "profileText"
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    text: profiles.displayText
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    verticalAlignment: Text.AlignVCenter
                    color: profiles.enabled ? Theme.text : Tokens.textDisabled
                }
            }
            background: Rectangle {
                implicitWidth: 150
                implicitHeight: Tokens.controlHeight
                radius: height / 2
                color: profiles.down ? Tokens.surface4 : profiles.hovered ? Tokens.surface3 : Theme.surface
                border.width: profiles.visualFocus ? 2 : 1
                border.color: profiles.visualFocus ? Theme.focus : Theme.fieldBorder
            }
            popup: T.Popup {
                id: profilePopup
                objectName: "profileChooserPopup"
                readonly property real footerHeight: profileSeparator.implicitHeight + manageProfilesItem.implicitHeight
                y: profiles.height + 4
                width: Math.min(Math.max(profiles.width, manageProfilesItem.implicitWidth + leftPadding + rightPadding, 220),
                                Math.max(220, root.width - 16))
                height: Math.min(contentItem.implicitHeight + topPadding + bottomPadding,
                                 9 * Tokens.controlHeight + footerHeight + topPadding + bottomPadding,
                                 (profiles.Window.height > 0 ? profiles.Window.height : 780) - topMargin - bottomMargin)
                topMargin: 6
                bottomMargin: 6
                leftMargin: 6
                rightMargin: 6
                padding: 4
                contentItem: ColumnLayout {
                    spacing: 0
                    ListView {
                        id: profilePopupList
                        objectName: "profilePopupList"
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        implicitHeight: Math.min(contentHeight, 9 * Tokens.controlHeight)
                        clip: true
                        model: root.library.profiles
                        currentIndex: profiles.highlightedIndex
                        highlightMoveDuration: 0
                        delegate: ItemDelegate {
                            id: profile
                            objectName: "profileOption"
                            required property var modelData
                            required property int index
                            width: ListView.view ? ListView.view.width : profilePopupList.width
                            highlighted: profiles.highlightedIndex === index && !manageProfilesItem.activeFocus
                            hoverEnabled: profiles.hoverEnabled
                            text: modelData.name + (modelData.id === root.library.activeProfileId ? qsTr("  · active") : "")
                            Accessible.name: text
                            ToolTip.visible: hovered || visualFocus
                            ToolTip.text: text
                            contentItem: Label { text: profile.text; textFormat: Text.PlainText; elide: Text.ElideRight }
                            onClicked: {
                                root.library.viewProfile(modelData.id)
                                profilePopup.close()
                            }
                        }
                        T.ScrollBar.vertical: ScrollBar {
                            policy: profilePopupList.contentHeight > profilePopupList.height ? T.ScrollBar.AlwaysOn : T.ScrollBar.AlwaysOff
                        }
                    }
                    MenuSeparator {
                        id: profileSeparator
                        objectName: "profilePopupSeparator"
                        Layout.fillWidth: true
                    }
                    ItemDelegate {
                        id: manageProfilesItem
                        objectName: "manageProfilesMenuItem"
                        Layout.fillWidth: true
                        enabled: root.library.canEditProfiles
                        hoverEnabled: profiles.hoverEnabled
                        highlighted: activeFocus
                        text: qsTr("Manage profiles")
                        icon.source: Theme.icon("edit")
                        icon.color: enabled ? Theme.text : Tokens.textDisabled
                        Accessible.name: text
                        Accessible.role: Accessible.Button
                        Accessible.onPressAction: if (enabled) clicked()
                        onClicked: {
                            if (!enabled) return
                            profilePopup.close()
                            root.profilesRequested()
                        }
                        Keys.onPressed: event => {
                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                event.accepted = true
                                if (manageProfilesItem.enabled) manageProfilesItem.clicked()
                            } else if (event.key === Qt.Key_Up || event.key === Qt.Key_Backtab
                                       || (event.key === Qt.Key_Tab && (event.modifiers & Qt.ShiftModifier))) {
                                profiles.forceActiveFocus(Qt.BacktabFocusReason)
                                event.accepted = true
                            } else if (event.key === Qt.Key_Tab) {
                                const next = profiles.nextItemInFocusChain(true)
                                profilePopup.close()
                                if (next) next.forceActiveFocus(Qt.TabFocusReason)
                                event.accepted = true
                            } else if (event.key === Qt.Key_Escape) {
                                profilePopup.close()
                                profiles.forceActiveFocus(Qt.ShortcutFocusReason)
                                event.accepted = true
                            }
                        }
                    }
                }
                background: Rectangle {
                    radius: Tokens.radius
                    color: Tokens.surface2
                    border.color: Tokens.borderStrong
                }
                enter: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Tokens.fast } }
            }
            ToolTip {
                visible: profiles.hovered && !profiles.down && !profiles.popup.visible
                text: profiles.displayText ? qsTr("Viewing profile: %1").arg(profiles.displayText) : profiles.Accessible.description
                width: Math.min(440, implicitWidth)
            }
        }
        TextMetrics { id: profileMetrics; font: profiles.font; text: profiles.displayText }
        StatusPill {
            visible: root.differentLaunchProfile
            Layout.maximumWidth: root.width * 0.2
            text: qsTr("Play uses %1").arg(root.launchTarget)
            tone: "accent"
        }
        Item { Layout.fillWidth: true; Layout.minimumWidth: 0 }
        Button {
            id: stopButton
            text: qsTr("Stop…")
            icon.source: Theme.icon("stop")
            visible: root.library.launch.canStop
            onClicked: root.launchDetailsRequested()
        }
        Row {
            // Bound to the model, not to launchButton.visible: a child of a
            // hidden parent reports hidden, which would never recover.
            visible: root.library.nativeMode && !root.library.readOnly
            spacing: 4
            Button {
                id: launchButton
                objectName: "launchActiveButton"
                width: Math.min(implicitWidth, Math.max(140, root.width * 0.3))
                text: root.launchProgress || qsTr("Play")
                enabled: root.library.launch.canLaunch
                highlighted: true
                leftPadding: 18
                rightPadding: 18
                icon.source: Theme.icon("play")
                icon.width: 14
                icon.height: 14
                icon.color: enabled ? Theme.selectionText : Theme.faint
                font.family: Theme.displayFamily
                font.pointSize: Theme.title
                font.weight: Font.DemiBold
                font.letterSpacing: 1.5
                font.capitalization: Font.AllUppercase
                Accessible.name: root.launchProgress ? qsTr("%1 · %2").arg(root.launchProgress).arg(root.launchTarget) : qsTr("Play with %1").arg(root.launchTarget)
                onVisibleChanged: if (!visible) { root.resetPlayIgnition(); playPlaque.stopIgnition() }
                contentItem: Item {
                    implicitWidth: playContentRow.implicitWidth
                    implicitHeight: playContentRow.implicitHeight
                    Row {
                        id: playContentRow
                        anchors.centerIn: parent
                        spacing: launchButton.spacing
                        Item {
                            width: 14
                            height: 14
                            anchors.verticalCenter: parent.verticalCenter
                            ColorImage {
                                id: playGlyphIcon
                                objectName: "playGlyphIcon"
                                anchors.fill: parent
                                source: Theme.icon("play")
                                sourceSize: Qt.size(14, 14)
                                color: launchButton.enabled ? Theme.selectionText : Theme.faint
                                Accessible.ignored: true
                            }
                            ColorImage {
                                id: playSunburstIcon
                                objectName: "playSunburstIcon"
                                anchors.fill: parent
                                source: Theme.icon("sunburst")
                                sourceSize: Qt.size(14, 14)
                                color: Theme.brass
                                opacity: 0
                                rotation: -180
                                scale: 0.65
                                Accessible.ignored: true
                            }
                        }
                        Label {
                            objectName: "playButtonLabel"
                            anchors.verticalCenter: parent.verticalCenter
                            text: launchButton.text
                            font: launchButton.font
                            color: launchButton.enabled ? Theme.selectionText : Theme.faint
                            Accessible.ignored: true
                        }
                    }
                }
                SequentialAnimation {
                    id: ignitionGlyphAnim
                    objectName: "playIgnitionGlyphAnimation"
                    loops: 1
                    onRunningChanged: if (!running) root.resetPlayIgnition()
                    ParallelAnimation {
                        NumberAnimation { target: playGlyphIcon; property: "rotation"; from: 0; to: 180; duration: Math.max(1, Tokens.fast); easing.type: Easing.InOutCubic }
                        NumberAnimation { target: playGlyphIcon; property: "opacity"; from: 1; to: 0; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutQuad }
                        NumberAnimation { target: playGlyphIcon; property: "scale"; from: 1; to: 0.65; duration: Math.max(1, Tokens.fast); easing.type: Easing.InQuad }
                        NumberAnimation { target: playSunburstIcon; property: "rotation"; from: -180; to: 0; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutBack }
                        NumberAnimation { target: playSunburstIcon; property: "opacity"; from: 0; to: 1; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutQuad }
                        NumberAnimation { target: playSunburstIcon; property: "scale"; from: 0.65; to: 1.1; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutCubic }
                    }
                    NumberAnimation { target: playSunburstIcon; property: "scale"; from: 1.1; to: 1.0; duration: Math.max(1, Tokens.fast); easing.type: Easing.OutCubic }
                    PauseAnimation { duration: Math.max(0, Tokens.fast) }
                    ParallelAnimation {
                        NumberAnimation { target: playSunburstIcon; property: "rotation"; from: 0; to: 180; duration: Math.max(1, Tokens.normal); easing.type: Easing.InOutCubic }
                        NumberAnimation { target: playSunburstIcon; property: "opacity"; from: 1; to: 0; duration: Math.max(1, Tokens.normal); easing.type: Easing.InQuad }
                        NumberAnimation { target: playSunburstIcon; property: "scale"; from: 1.0; to: 0.65; duration: Math.max(1, Tokens.normal); easing.type: Easing.InQuad }
                        NumberAnimation { target: playGlyphIcon; property: "rotation"; from: 180; to: 360; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutBack }
                        NumberAnimation { target: playGlyphIcon; property: "opacity"; from: 0; to: 1; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutQuad }
                        NumberAnimation { target: playGlyphIcon; property: "scale"; from: 0.65; to: 1.0; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutCubic }
                    }
                }
                background: DecoPlayPlaque {
                    id: playPlaque
                    implicitHeight: 36
                    implicitWidth: 112
                    chamferLeft: true
                    chamferRight: false
                    activeState: launchButton.enabled
                    hovered: launchButton.hovered
                    down: launchButton.down
                    sheenActive: root.playPendingChanges
                    Rectangle { objectName: "playOuterFocusRing"; anchors.fill: parent; anchors.margins: -4; radius: 4; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: launchButton.visualFocus }
                    Rectangle { objectName: "playFillFocusRing"; anchors.fill: parent; radius: 2; color: "transparent"; border.width: 2; border.color: Theme.selectionText; visible: launchButton.visualFocus }
                }
                ToolTip {
                    visible: launchButton.hovered || launchButton.visualFocus
                    text: qsTr("Start Deadlock with the active profile: %1").arg(root.launchTarget)
                    width: Math.min(440, implicitWidth)
                }
                onClicked: root.library.launch.launchActive("modded")
            }
            Button {
                id: launchMenuButton
                objectName: "launchMenuButton"
                text: qsTr("Options")
                display: AbstractButton.IconOnly
                icon.source: Theme.icon("chevron-down")
                icon.color: enabled ? Theme.selectionText : Theme.faint
                Accessible.name: qsTr("Play options")
                enabled: root.library.launch.available
                highlighted: true
                leftPadding: 8
                rightPadding: 14
                background: DecoPlayPlaque {
                    implicitHeight: 36
                    implicitWidth: 36
                    chamferLeft: false
                    chamferRight: true
                    activeState: launchMenuButton.enabled
                    hovered: launchMenuButton.hovered
                    down: launchMenuButton.down
                    sheenActive: false
                    Rectangle { objectName: "playOuterFocusRing"; anchors.fill: parent; anchors.margins: -4; radius: 4; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: launchMenuButton.visualFocus }
                    Rectangle { objectName: "playFillFocusRing"; anchors.fill: parent; radius: 2; color: "transparent"; border.width: 2; border.color: Theme.selectionText; visible: launchMenuButton.visualFocus }
                }
                onClicked: launchMenu.popup(launchMenuButton, launchMenuButton.width - launchMenu.width, launchMenuButton.height + 4)
                Menu {
                    id: launchMenu
                    onAboutToShow: root.suppressLaunchFocus = false
                    MenuItem { text: qsTr("Play without mods"); icon.source: Theme.icon("play"); enabled: root.library.launch.canLaunch; onTriggered: root.library.launch.launchActive("vanilla") }
                    MenuItem { text: qsTr("Stop waiting"); visible: root.library.launch.canStopWaiting; onTriggered: root.library.launch.cancel() }
                    MenuSeparator {}
                    MenuItem { text: qsTr("Launch settings…"); icon.source: Theme.icon("settings"); onTriggered: { root.suppressLaunchFocus = true; root.launchSettingsRequested() } }
                    MenuItem { objectName: "launchDetailsMenuItem"; text: qsTr("Launch details…"); icon.source: Theme.icon("info"); onTriggered: { root.suppressLaunchFocus = true; root.launchDetailsRequested() } }
                    onClosed: if (!root.suppressLaunchFocus) Qt.callLater(root.focusLaunch)
                }
            }
        }
        ToolButton {
            id: moreButton
            objectName: "moreButton"
            text: qsTr("More")
            display: AbstractButton.IconOnly
            icon.source: Theme.icon("more")
            Accessible.name: qsTr("More actions")
            ToolTip { visible: moreButton.hovered || moreButton.visualFocus; text: qsTr("More actions") }
            onClicked: more.popup(moreButton, moreButton.width - more.width, moreButton.height + 4)
            Menu {
                id: more
                onAboutToShow: root.suppressMoreFocus = false
                MenuItem { text: qsTr("Game setup"); enabled: false; contentItem: Label { text: parent.text; color: Theme.faint; leftPadding: 8 } }
                MenuItem { objectName: "applyButton"; text: qsTr("Apply changes…"); icon.source: Theme.icon("check"); visible: root.library.nativeMode; enabled: root.library.canPreviewApply; onTriggered: { root.suppressMoreFocus = true; root.applyRequested() } }
                MenuItem { objectName: "installOrderButton"; text: qsTr("Load order…"); icon.source: Theme.icon("sort"); visible: root.library.nativeMode; enabled: root.library.options.available; onTriggered: { root.suppressMoreFocus = true; root.library.options.openOrder() } }
                MenuItem { objectName: "restoreButton"; text: qsTr("Restore previous setup…"); icon.source: Theme.icon("undo"); visible: root.library.nativeMode; enabled: root.library.canRestore; onTriggered: { root.suppressMoreFocus = true; root.restoreRequested() } }
                MenuSeparator {}
                MenuItem { text: qsTr("Library"); enabled: false; contentItem: Label { text: parent.text; color: Theme.faint; leftPadding: 8 } }
                MenuItem { objectName: "refreshButton"; text: qsTr("Reload"); icon.source: Theme.icon("refresh"); enabled: !root.library.demoMode && !root.library.loading && !root.library.busy; onTriggered: root.library.refresh() }
                MenuItem {
                    objectName: "createLibraryButton"
                    text: qsTr("Create library")
                    visible: !root.library.demoMode && !root.library.nativeMode && root.library.sourcePath !== "" && root.library.bootstrapState === "absent"
                    enabled: root.library.canCreateLibrary
                    Accessible.description: qsTr("Creates an empty library in this app. Your Deadlock Mod Manager data stays unchanged.")
                    onTriggered: root.library.createLibrary()
                }
                MenuItem {
                    objectName: "importButton"
                    text: qsTr("Import from Deadlock Mod Manager…")
                    visible: !root.library.demoMode && (!root.library.nativeMode || root.library.readOnly)
                    enabled: root.library.canImport
                    onTriggered: { root.suppressMoreFocus = true; root.importRequested() }
                }
                MenuItem { text: qsTr("Import history…"); icon.source: Theme.icon("package"); visible: root.library.nativeMode; onTriggered: { root.suppressMoreFocus = true; root.library.options.showHistory() } }
                MenuItem { text: qsTr("Downloads…"); visible: false; enabled: root.library.remote.available && root.downloadsAllowed; onTriggered: { root.suppressMoreFocus = true; root.downloadsRequested() } }
                MenuItem { text: qsTr("Game fonts…"); icon.source: Theme.icon("edit"); visible: root.library.nativeMode; enabled: root.library.fonts.available && !root.library.fonts.busy; onTriggered: { root.suppressMoreFocus = true; root.fontReturnToMore = true; root.library.fonts.showInstalled(); if (!root.library.fonts.managerVisible) root.fontReturnToMore = false } }
                MenuItem { text: qsTr("Undo last font change…"); icon.source: Theme.icon("undo"); visible: root.library.nativeMode; enabled: root.library.fonts.canUndo; onTriggered: { root.fontReturnToMore = true; root.library.fonts.previewUndo(); if (!root.library.fonts.visible) root.fontReturnToMore = false } }
                MenuSeparator {}
                MenuItem { text: qsTr("Advanced"); enabled: false; contentItem: Label { text: parent.text; color: Theme.faint; leftPadding: 8 } }
                MenuItem { objectName: "chooseSourceButton"; text: qsTr("Open a library file from the original manager…"); icon.source: Theme.icon("folder"); enabled: !root.library.loading && !root.library.busy && !root.library.outcomeUnknown && !root.library.recoveryRequired; onTriggered: { root.suppressMoreFocus = true; root.openSourceRequested() } }
                MenuSeparator {}
                MenuItem {
                    text: qsTr("Performance overlay")
                    checkable: true
                    checked: root.performanceVisible
                    onTriggered: root.performanceVisible = !root.performanceVisible
                    Accessible.description: qsTr("Show rendering diagnostics and the animation test")
                }
                onClosed: if (!root.suppressMoreFocus) Qt.callLater(root.focusMore)
            }
        }
    }
}
