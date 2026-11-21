pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Templates as T
import QtQuick.Layouts
import Deadlock.Native

// Game tweaks that live in the game's own config files. Choosing something here
// uses the existing automatic write transaction; Restore undoes it.
Item {
    id: root
    required property LibraryModel library
    readonly property bool popupOpen: reviewLoader.active
    signal applyRequested()
    readonly property PerformanceController perf: library.performance
    readonly property var presets: perf ? perf.presets : []
    readonly property var detected: perf ? perf.detected : ({})
    readonly property string selection: perf ? perf.selection : ""
    property string focusedId: ""
    property int tab: 0
    property var reviewController: null
    function openReview(controller) { if (controller.preview()) { reviewController = controller; reviewLoader.active = true } }
    readonly property string shownId: focusedId !== "" ? focusedId : selection
    readonly property var shownPreset: presets.find(p => p.id === shownId) || null
    readonly property var gamePreset: presets.find(p => p.id === detected.presetId) || null
    // The game has a preset from the old manager that the library hasn't chosen.
    readonly property bool keepOffer: detected.state === "preset" && !!gamePreset && selection === ""
    readonly property bool differsFromGame: detected.state === "preset" ? selection !== (detected.presetId || "")
                                          : selection !== ""

    function presetName(id) { const p = presets.find(x => x.id === id); return p ? p.name : "" }
    function gameStateText() {
        switch (detected.state) {
        case "stock": return qsTr("No performance config. The game uses Valve's settings.")
        case "preset": return gamePreset ? qsTr("Uses “%1”.").arg(gamePreset.name) + (detected.handEdited ? qsTr(" Some values were changed by hand.") : "")
                                         : qsTr("Uses a performance config this app doesn't recognise.")
        case "custom": return qsTr("Has custom performance edits made outside this app.")
        case "foreign": return qsTr("Has settings from another tool. Remove them with that tool before applying from here.")
        case "unavailable": return qsTr("Choose Deadlock's folder to use performance presets.")
        case "unreadable": return qsTr("The game's settings file couldn't be read.")
        default: return qsTr("Checking the game…")
        }
    }

    component PresetCard: T.AbstractButton {
        id: card
        property string presetId
        objectName: "performancePreset_" + presetId
        property string title
        property string subtitle
        property string body
        readonly property bool chosen: root.selection === presetId && !(presetId === "" && root.keepOffer)
        readonly property bool shown: root.shownId === presetId
        Layout.fillWidth: true
        implicitHeight: cardColumn.implicitHeight + Theme.spacing24
        focusPolicy: Qt.StrongFocus
        onClicked: root.focusedId = presetId
        ToolTip.visible: hovered || activeFocus
        ToolTip.text: [title, subtitle, body].filter(Boolean).join("\n")
        background: Rectangle {
            radius: Theme.radiusCard
            color: card.chosen ? Theme.accentSoft : card.hovered ? Theme.surfaceHover : Theme.surfaceLow
            border.width: card.chosen ? 2 : 1
            border.color: card.chosen ? Theme.accentRing : Theme.rule
            Rectangle { anchors.fill: parent; anchors.margins: -3; radius: Theme.radiusCard + 3; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: card.visualFocus }
        }
        Accessible.role: Accessible.RadioButton
        Accessible.name: [title, subtitle, body, chosen ? qsTr("In use") : ""].filter(Boolean).join(" · ")
        Accessible.checked: chosen
        RowLayout {
            id: cardColumn
            anchors.fill: parent
            anchors.margins: Theme.spacing12
            spacing: Theme.spacing12
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                RowLayout {
                    Layout.fillWidth: true
                    Label { text: card.title; font.weight: Font.DemiBold; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    StatusPill { visible: card.chosen; text: qsTr("In use"); tone: "accent" }
                }
                Label { visible: card.subtitle !== ""; text: card.subtitle; color: Theme.faint; font.pointSize: Theme.small; textFormat: Text.PlainText; Layout.fillWidth: true; elide: Text.ElideRight }
                Label { visible: card.body !== ""; text: card.body; color: Theme.secondary; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true; maximumLineCount: 3; elide: Text.ElideRight }
            }

            Button {
                objectName: "choosePerformancePreset_" + card.presetId
                Layout.alignment: Qt.AlignVCenter
                visible: !card.chosen
                text: qsTr("Choose")
                enabled: !!root.perf && root.perf.available && !root.perf.busy
                onClicked: { root.focusedId = card.presetId; root.perf.select(card.presetId) }
            }

        }
    }

    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            width: Math.min(parent.width - 48, 1100)
            x: 24
            spacing: Theme.spacing16
            Item { Layout.preferredHeight: Theme.spacing8 }
            SectionHeading { text: qsTr("Tweaks"); pointSize: Theme.heading }
            TabBar {
                id: tabs
                Layout.fillWidth: true
                currentIndex: root.tab
                onCurrentIndexChanged: root.tab = currentIndex
                TabButton { width: implicitWidth; text: qsTr("Performance"); icon.source: Theme.icon("sliders") }
                TabButton { width: implicitWidth; text: qsTr("Crosshair"); icon.source: Theme.icon("zoom-in") }
                TabButton { width: implicitWidth; text: qsTr("Autoexec"); icon.source: Theme.icon("edit") }
            }
            CrosshairPanel {
                Layout.fillWidth: true
                visible: root.tab === 1
                controller: root.library.crosshair
                overriddenByAutoexec: (root.library.autoexec.managedSections || []).some(sec => /Crosshair Settings/.test(sec.content || ""))
                onReviewRequested: { root.reviewController = root.library.crosshair; reviewLoader.active = true }
            }
            AutoexecPanel {
                Layout.fillWidth: true
                visible: root.tab === 2
                controller: root.library.autoexec
                onReviewRequested: { root.reviewController = root.library.autoexec; reviewLoader.active = true }
            }
            ColumnLayout {
            Layout.fillWidth: true
            visible: root.tab === 0
            spacing: Theme.spacing16
            Label {
                Layout.fillWidth: true
                text: qsTr("Choose a performance preset. Changes are saved when Deadlock is closed, or when you next play.")
                color: Theme.secondary
                wrapMode: Text.Wrap
            }

            Label {
                Layout.fillWidth: true
                visible: !root.perf || !root.perf.available
                text: root.library.nativeMode ? qsTr("Performance configs need a newer helper.") : qsTr("Import or create a library to use tweaks.")
                color: Theme.warning
                wrapMode: Text.Wrap
            }

            // Current game state and the "keep it" offer.
            Rectangle {
                Layout.fillWidth: true
                visible: !!root.perf && root.perf.available && (root.keepOffer || ["foreign", "custom", "unreadable"].includes(root.detected.state))
                implicitHeight: stateRow.implicitHeight + Theme.spacing24
                radius: Theme.radius
                color: root.keepOffer || root.detected.state === "foreign" ? Theme.warningSoft : Theme.surfaceLow
                border.color: Theme.rule
                RowLayout {
                    id: stateRow
                    anchors.fill: parent
                    anchors.margins: Theme.spacing12
                    spacing: Theme.spacing12
                    IconImage { source: Theme.icon(root.keepOffer || root.detected.state === "foreign" ? "alert" : "info"); sourceSize: Qt.size(20, 20); color: root.keepOffer || root.detected.state === "foreign" ? Theme.warning : Theme.secondary; Layout.alignment: Qt.AlignTop }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label { text: qsTr("Existing settings"); font.weight: Font.DemiBold }
                        Label { Layout.fillWidth: true; text: root.gameStateText(); wrapMode: Text.Wrap; textFormat: Text.PlainText }
                        Label {
                            Layout.fillWidth: true
                            visible: root.keepOffer
                            text: qsTr("Keep this preset from the previous Mod Manager?")
                            wrapMode: Text.Wrap
                            color: Theme.secondary
                        }
                    }
                    Button {
                        visible: root.keepOffer
                        highlighted: true
                        text: qsTr("Keep “%1”").arg(root.gamePreset ? root.gamePreset.name : "")
                        enabled: root.perf.available && !root.perf.busy
                        onClicked: root.perf.select(root.detected.presetId)
                    }

                }
            }
            Label {
                Layout.fillWidth: true
                visible: !!root.perf && root.perf.error !== ""
                text: Theme.friendlyError(root.perf ? root.perf.error : "")
                color: Theme.danger
                wrapMode: Text.Wrap
                textFormat: Text.PlainText
            }

            GridLayout {
                Layout.fillWidth: true
                visible: !!root.perf && root.perf.available
                columns: root.width < 900 ? 1 : 2
                columnSpacing: Theme.spacing16
                rowSpacing: Theme.spacing16
                ColumnLayout {
                    Layout.fillWidth: root.width < 900
                    Layout.preferredWidth: 420
                    Layout.maximumWidth: root.width < 900 ? Number.POSITIVE_INFINITY : 480
                    Layout.alignment: Qt.AlignTop
                    spacing: Theme.spacing8
                    PresetCard { presetId: ""; title: qsTr("None"); body: qsTr("Use Valve's own settings.") }
                    Repeater {
                        model: root.presets
                        PresetCard {
                            required property var modelData
                            presetId: modelData.id
                            title: modelData.name
                            subtitle: [Theme.meaningful(modelData.author) ? qsTr("by %1").arg(modelData.author) : "",
                                       modelData.entryCount === 1 ? qsTr("1 change") : qsTr("%1 changes").arg(modelData.entryCount)].filter(t => t !== "").join(" · ")
                            body: modelData.description || ""
                        }
                    }
                }
                // What the focused preset changes.
                Rectangle {
                    objectName: "performanceSummary"
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredHeight: Math.max(240, changes.implicitHeight + Theme.spacing24)
                    radius: Theme.radius
                    color: Theme.surfaceLow
                    border.color: Theme.rule
                    ColumnLayout {
                        id: changes
                        property bool showEntries: false
                        property bool showOmitted: false
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: Theme.spacing12
                        spacing: Theme.spacing8
                        readonly property var groupLabels: {
                            const m = {}
                            for (const g of (root.shownPreset ? root.shownPreset.categoryGroups || [] : [])) m[g.id] = g.label
                            return m
                        }
                        // Sorted by category so each group gets one heading.
                        readonly property var entries: root.shownPreset ? (root.perf.preview(root.shownPreset.id) || []).filter(e => e.allowed)
                            .slice().sort((a, b) => (groupLabels[a.category] || a.category || "").localeCompare(groupLabels[b.category] || b.category || "")) : []
                        readonly property var omittedEntries: root.shownPreset ? (root.perf.preview(root.shownPreset.id) || []).filter(e => !e.allowed) : []
                        readonly property int omitted: omittedEntries.length
                        Label {
                            Layout.fillWidth: true
                            text: root.shownPreset ? qsTr("What “%1” changes").arg(root.shownPreset.name) : qsTr("Nothing changes")
                            font.weight: Font.DemiBold
                            wrapMode: Text.Wrap
                            textFormat: Text.PlainText
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: !root.shownPreset
                            text: qsTr("Use Valve's settings.")
                            color: Theme.secondary
                            wrapMode: Text.Wrap
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: !!root.shownPreset && !!root.shownPreset.description
                            text: root.shownPreset ? root.shownPreset.description || "" : ""
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: !!root.shownPreset && !!Theme.meaningful(root.shownPreset.author)
                            text: root.shownPreset ? qsTr("by %1").arg(root.shownPreset.author || "") : ""
                            color: Theme.secondary
                            textFormat: Text.PlainText
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: changes.omitted > 0
                            text: changes.omitted === 1 ? qsTr("1 unsupported setting is skipped.")
                                                        : qsTr("%1 unsupported settings are skipped.").arg(changes.omitted)
                            color: Theme.secondary
                            font.pointSize: Theme.small
                            wrapMode: Text.Wrap
                        }
                        Disclosure {
                            objectName: "performanceSkippedToggle"
                            visible: changes.omitted > 0
                            label: changes.showOmitted ? qsTr("Hide skipped settings") : qsTr("Show skipped settings")
                            checked: changes.showOmitted
                            onToggled: changes.showOmitted = checked
                        }
                        Repeater {
                            objectName: "performanceSkippedList"
                            model: changes.showOmitted ? changes.omittedEntries : []
                            Label {
                                required property var modelData
                                Layout.fillWidth: true
                                text: modelData.label || modelData.key
                                textFormat: Text.PlainText
                                wrapMode: Text.Wrap
                                color: Theme.secondary
                            }
                        }
                        Disclosure {
                            objectName: "performanceSettingsToggle"
                            visible: !!root.shownPreset
                            label: changes.showEntries ? qsTr("Hide settings") : qsTr("Show all settings (%1)").arg(changes.entries.length)
                            checked: changes.showEntries
                            onToggled: changes.showEntries = checked
                        }
                        Repeater {
                            objectName: "performanceSettingsList"
                            model: changes.showEntries ? changes.entries : []
                            ColumnLayout {
                                id: entry
                                required property var modelData
                                required property int index
                                readonly property string group: changes.groupLabels[modelData.category] || modelData.category || qsTr("Other")
                                readonly property bool firstInGroup: index === 0 || (changes.groupLabels[changes.entries[index - 1].category] || changes.entries[index - 1].category || qsTr("Other")) !== group
                                readonly property string fromText: modelData.from === null || modelData.from === undefined ? qsTr("default") : modelData.from
                                readonly property string toText: modelData.to === null || modelData.to === undefined ? qsTr("removed") : modelData.to
                                Layout.fillWidth: true
                                spacing: 0
                                Label {
                                    visible: entry.firstInGroup
                                    Layout.topMargin: entry.index === 0 ? 0 : Theme.spacing12
                                    Layout.bottomMargin: 4
                                    text: entry.group
                                    font.weight: Font.DemiBold
                                    color: Theme.accentInk
                                    textFormat: Text.PlainText
                                }
                                Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Theme.rule; visible: !entry.firstInGroup }
                                RowLayout {
                                    Layout.fillWidth: true
                                    Layout.topMargin: Theme.spacing8
                                    Label {
                                        Layout.fillWidth: true
                                        text: entry.modelData.label || entry.modelData.key
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        font.weight: Font.Medium
                                    }
                                    Label {
                                        // Values already in the game show once; real changes show before → after.
                                        text: entry.fromText === entry.toText ? entry.toText : entry.fromText + "  →  " + entry.toText
                                        textFormat: Text.PlainText
                                        color: Theme.secondary
                                        font.family: "monospace"
                                    }
                                }
                                Label {
                                    Layout.fillWidth: true
                                    readonly property bool useful: !!entry.modelData.description && !/doesn.t document/i.test(entry.modelData.description)
                                    visible: useful
                                    text: useful ? entry.modelData.description : ""
                                    textFormat: Text.PlainText
                                    wrapMode: Text.Wrap
                                    color: Theme.faint
                                    font.pointSize: Theme.small
                                }
                            }
                        }
                    }
                }
            }
            }
            Item { Layout.preferredHeight: Theme.spacing24 }
        }
    }
    Loader {
        id: reviewLoader
        active: false
        sourceComponent: ConfigReviewDialog {
            controller: root.reviewController
            heading: root.reviewController === root.library.crosshair ? qsTr("Save crosshair") : qsTr("Save autoexec.cfg")
            Component.onCompleted: open()
            onClosed: reviewLoader.active = false
        }
    }
}
