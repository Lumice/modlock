pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// Start "Find a broken mod": explain the steps and pick the mods to test.
// The search itself only switches mods on and off in the active profile;
// the game changes when the user plays (the usual Apply step).
Dialog {
    id: root
    required property LibraryModel library
    property var mods: []
    property var selected: ({})
    readonly property var selectedIds: mods.filter(m => selected[m.id]).map(m => m.id)
    readonly property int launches: selectedIds.length < 2 ? 1 : Math.ceil(Math.log2(selectedIds.length)) + 2
    property string startError: ""

    function load() {
        const rows = []
        const next = {}
        for (let i = 0; i < library.count; ++i) {
            const id = library.idAt(i)
            if (!id) continue
            const m = library.modDetails(id)
            rows.push({ id: id, title: m.title || id, enabled: !!m.enabled })
            if (m.enabled) next[id] = true
        }
        mods = rows
        selected = next
    }
    function toggle(id, on) { const next = Object.assign({}, selected); if (on) next[id] = true; else delete next[id]; selected = next }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(640, parent ? parent.width - 40 : 640)
    height: Math.min(parent ? parent.height - 40 : 640, 680)
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    title: qsTr("Find a broken mod")
    Component.onCompleted: { load(); open() }

    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: qsTr("If something in the game is broken and you don't know which mod causes it, the app can narrow it down for you.")
            wrapMode: Text.Wrap
        }
        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Theme.spacing8
            rowSpacing: 6
            Repeater {
                model: [
                    qsTr("The app turns half of the chosen mods off. Nothing is deleted or downloaded again."),
                    qsTr("Press Play, check whether the problem is still there, then close the game."),
                    qsTr("Answer \"Still broken\" or \"Works now\". Each answer rules out half of the mods."),
                    qsTr("When it's found, your mods come back as they were. You choose whether the broken one stays off.")
                ]
                delegate: RowLayout {
                    id: step
                    required property string modelData
                    required property int index
                    Layout.columnSpan: 2
                    Layout.fillWidth: true
                    spacing: Theme.spacing8
                    Rectangle {
                        Layout.alignment: Qt.AlignTop
                        width: 22; height: 22; radius: 11
                        color: Theme.accentSoft
                        Label { anchors.centerIn: parent; text: String(step.index + 1); color: Theme.accentInk; font.weight: Font.DemiBold; font.pointSize: Theme.small }
                    }
                    Label { Layout.fillWidth: true; text: step.modelData; wrapMode: Text.Wrap; color: Theme.secondary }
                }
            }
        }
        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                text: qsTr("Mods to test (%1 of %2)").arg(root.selectedIds.length).arg(root.mods.length)
                font.weight: Font.DemiBold
            }
            SegmentedControl {
                model: [{text: qsTr("Enabled only"), value: "enabled"}, {text: qsTr("All"), value: "all"}]
                currentValue: root.mods.length && root.mods.every(m => !!root.selected[m.id]) ? "all"
                    : root.mods.length && root.mods.every(m => !!root.selected[m.id] === m.enabled) ? "enabled" : ""
                onActivated: value => {
                    const next = {}
                    for (const m of root.mods) if (value === "all" || m.enabled) next[m.id] = true
                    root.selected = next
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.mods.length === 0
            text: qsTr("There are no mods in this profile yet.")
            color: Theme.secondary
        }
        ListView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 120
            clip: true
            model: root.mods
            ScrollBar.vertical: ScrollBar {}
            delegate: CheckBox {
                required property var modelData
                width: ListView.view.width
                text: modelData.title + (modelData.enabled ? "" : qsTr("  (off now)"))
                checked: !!root.selected[modelData.id]
                onToggled: root.toggle(modelData.id, checked)
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.selectedIds.length > 0
            text: root.selectedIds.length < 2 ? qsTr("With one mod there's nothing to narrow down: turn it off and check.")
                : qsTr("Usually about %1 launches. If two mods only break together, it can take a few more.").arg(root.launches)
            color: Theme.secondary
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: root.startError !== ""
            text: root.startError
            color: Theme.danger
            wrapMode: Text.Wrap
            textFormat: Text.PlainText
        }
        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            Button { text: qsTr("Cancel"); onClicked: root.close() }
            Button {
                objectName: "bisectStart"
                highlighted: true
                text: qsTr("Start")
                enabled: root.selectedIds.length >= 2 && root.library.canStartBisect && !root.library.busy
                onClicked: {
                    root.startError = ""
                    if (root.library.startBisect(root.selectedIds)) root.close()
                    else root.startError = qsTr("The search can't start right now. Finish any running change and try again.")
                }
            }
        }
    }
}
