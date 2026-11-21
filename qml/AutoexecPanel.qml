pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "AutoexecCommands.js" as AutoexecCommands

// Plain-text editor for the game's autoexec.cfg. Sections another tool
// manages are shown separately and kept as they are.
ColumnLayout {
    id: root
    required property GameConfigController controller
    signal reviewRequested()
    property string draft: controller.text
    readonly property bool dirty: draft !== controller.text
    Connections { target: root.controller; function onChanged() { if (!root.dirty) root.draft = root.controller.text } }
    spacing: Theme.spacing12

    property string libraryCategory: "performance"
    property string librarySearch: ""
    // Commands already set in the draft (first word of each non-comment line).
    readonly property var draftCommands: {
        const found = {}
        for (const raw of root.draft.split("\n")) {
            const line = raw.trim()
            if (line === "" || line.startsWith("//")) continue
            found[line.split(/\s+/)[0].toLowerCase()] = true
        }
        return found
    }
    readonly property var libraryRows: {
        const q = root.librarySearch.trim().toLowerCase()
        const rows = []
        for (const c of AutoexecCommands.categories) {
            if (q === "" && c.id !== root.libraryCategory) continue
            for (const cmd of c.commands)
                if (q === "" || cmd.command.toLowerCase().includes(q) || cmd.description.toLowerCase().includes(q))
                    rows.push(Object.assign({ category: c.label }, cmd))
        }
        return rows
    }
    function addCommand(cmd) {
        const line = cmd.command + " " + cmd.value
        const text = root.draft
        root.draft = text === "" ? line + "\n" : text + (text.endsWith("\n") ? "" : "\n") + line + "\n"
    }

    Label {
        Layout.fillWidth: true
        text: qsTr("Console commands the game runs every time it starts, one per line, for example fov_desired 90. Review changes before saving to autoexec.cfg. A backup lets you undo the last save. Close Deadlock first.")
        color: Theme.secondary
        wrapMode: Text.Wrap
    }
    RowLayout {
        Layout.fillWidth: true
        visible: root.controller.path !== ""
        Label {
            Layout.fillWidth: true
            text: qsTr("autoexec.cfg in Deadlock's cfg folder")
            elide: Text.ElideRight
            color: Theme.secondary
            ToolTip.visible: pathHover.hovered
            ToolTip.text: root.controller.path
            HoverHandler { id: pathHover }
        }
        Button {
            text: qsTr("Open"); icon.source: Theme.icon("folder"); flat: true
            Accessible.description: root.controller.path
            ToolTip.visible: hovered || activeFocus
            ToolTip.text: root.controller.path
            onClicked: Qt.openUrlExternally("file://" + root.controller.path.substring(0, root.controller.path.lastIndexOf("/")).split("/").map(part => encodeURIComponent(part)).join("/"))
        }
    }
    ScrollView {
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(280, Math.max(140, root.Window.height - 380))
        TextArea {
            id: editor
            objectName: "autoexecEditor"
            text: root.draft
            font.family: "monospace"
            textFormat: TextEdit.PlainText
            wrapMode: TextEdit.NoWrap
            selectByMouse: true
            placeholderText: qsTr("// Nothing here yet")
            enabled: root.controller.available && !root.controller.busy
            Accessible.name: qsTr("autoexec.cfg")
            onTextChanged: if (root.draft !== text) root.draft = text
        }
    }
    RowLayout {
        Layout.fillWidth: true
        Button { text: qsTr("Discard changes"); flat: true; visible: root.dirty; onClicked: root.draft = root.controller.text }
        Label { Layout.fillWidth: true; visible: root.dirty; text: qsTr("Unsaved changes"); color: Theme.secondary }
        Item { Layout.fillWidth: true; visible: !root.dirty }
        Button {
            text: qsTr("Undo last save…")
            icon.source: Theme.icon("undo")
            visible: root.controller.canUndo
            enabled: !root.controller.busy
            onClicked: if (root.controller.undo()) root.reviewRequested()
        }
        Button {
            objectName: "autoexecSave"
            highlighted: true
            text: qsTr("Save to game…")
            icon.source: Theme.icon("check")
            enabled: root.controller.available && !root.controller.busy && root.dirty
            onClicked: if (root.controller.preview(root.draft)) root.reviewRequested()
        }
    }
    ColumnLayout {
        Layout.fillWidth: true
        visible: (root.controller.managedSections || []).length > 0
        spacing: 4
        Label { text: qsTr("Kept as is"); font.weight: Font.DemiBold }
        Label {
            Layout.fillWidth: true
            text: qsTr("These parts were added by the old Mod Manager. They stay in the file unchanged; saving a crosshair here replaces the old crosshair part.")
            color: Theme.secondary
            wrapMode: Text.Wrap
            font.pointSize: Theme.small
        }
        Repeater {
            model: root.controller.managedSections || []
            Rectangle {
                id: section
                required property var modelData
                property bool expanded: false
                readonly property var lines: String(section.modelData.content || "").replace(/\s+$/, "").split("\n")
                Layout.fillWidth: true
                implicitHeight: managedContent.implicitHeight + 16
                radius: Theme.radius
                color: Theme.surfaceLow
                border.color: Theme.rule
                ColumnLayout {
                    id: managedContent
                    x: 8; y: 8
                    width: parent.width - 16
                    Label {
                    id: sectionText
                    objectName: "managedSectionText"
                    Layout.fillWidth: true
                    text: section.expanded ? section.modelData.content : section.lines[0]
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    font.family: "monospace"
                    font.pointSize: Theme.small
                    color: Theme.secondary
                }
                Button {
                    id: sectionToggle
                    objectName: "managedSectionToggle"
                    flat: true
                    text: section.expanded ? qsTr("Show less") : qsTr("Show %1 lines").arg(section.lines.length)
                    onClicked: section.expanded = !section.expanded
                }
                }
            }
        }
    }
    // Command library
    ColumnLayout {
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacing8
        spacing: Theme.spacing8
        Label { text: qsTr("Command library"); font.weight: Font.DemiBold; font.pointSize: Theme.heading }
        Label {
            Layout.fillWidth: true
            text: qsTr("Common commands from the community. Add puts the line in the editor above; nothing changes in the game until you save.")
            color: Theme.secondary
            wrapMode: Text.Wrap
        }
        TextField {
            Layout.fillWidth: true
            placeholderText: qsTr("Search commands")
            text: root.librarySearch
            onTextEdited: root.librarySearch = text
            Accessible.name: qsTr("Search commands")
        }
        Flow {
            Layout.fillWidth: true
            spacing: 6
            visible: root.librarySearch.trim() === ""
            Repeater {
                model: AutoexecCommands.categories
                Button {
                    required property var modelData
                    // "&" marks a keyboard shortcut in button text; "&&" shows a literal "&".
                    text: modelData.label.replace(/&/g, "&&")
                    Accessible.name: modelData.label
                    checkable: true
                    checked: root.libraryCategory === modelData.id
                    flat: !checked
                    onClicked: root.libraryCategory = modelData.id
                }
            }
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: commandList.implicitHeight
            radius: Theme.radius
            color: Theme.surfaceLow
            border.color: Theme.rule
            Column {
                id: commandList
                width: parent.width
                Label {
                    visible: root.libraryRows.length === 0
                    width: parent.width
                    padding: Theme.spacing12
                    text: qsTr("No commands match.")
                    color: Theme.secondary
                }
                Repeater {
                    model: root.libraryRows
                    Item {
                        id: cmdRow
                        required property var modelData
                        required property int index
                        readonly property bool added: !!root.draftCommands[modelData.command.toLowerCase()]
                        width: commandList.width
                        implicitHeight: Math.max(cmdText.implicitHeight, addButton.implicitHeight) + Theme.spacing16
                        Rectangle { visible: cmdRow.index > 0; width: parent.width; height: 1; color: Theme.rule }
                        ColumnLayout {
                            id: cmdText
                            anchors { left: parent.left; right: addButton.left; verticalCenter: parent.verticalCenter; leftMargin: Theme.spacing12; rightMargin: Theme.spacing12 }
                            spacing: 2
                            RowLayout {
                                spacing: Theme.spacing8
                                Label {
                                    text: cmdRow.modelData.command + "  " + cmdRow.modelData.value
                                    font.family: "monospace"
                                    textFormat: Text.PlainText
                                }
                                StatusPill { visible: cmdRow.modelData.default; text: qsTr("Game default") }
                                Label {
                                    visible: root.librarySearch.trim() !== ""
                                    text: cmdRow.modelData.category
                                    color: Theme.faint
                                    font.pointSize: Theme.small
                                }
                            }
                            Label {
                                Layout.fillWidth: true
                                text: cmdRow.modelData.description
                                color: Theme.secondary
                                font.pointSize: Theme.small
                                wrapMode: Text.Wrap
                                textFormat: Text.PlainText
                            }
                        }
                        Button {
                            id: addButton
                            anchors { right: parent.right; verticalCenter: parent.verticalCenter; rightMargin: Theme.spacing12 }
                            text: cmdRow.added ? qsTr("Added") : qsTr("Add")
                            icon.source: Theme.icon(cmdRow.added ? "check" : "plus")
                            enabled: !cmdRow.added && root.controller.available && !root.controller.busy
                            Accessible.name: qsTr("Add %1").arg(cmdRow.modelData.command)
                            onClicked: root.addCommand(cmdRow.modelData)
                        }
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Valve can change or remove commands in any update. If something stops working after you add a command, remove it again.")
            color: Theme.faint
            font.pointSize: Theme.small
            wrapMode: Text.Wrap
        }
    }
    Label {
        Layout.fillWidth: true
        visible: root.controller.error !== ""
        text: root.controller.error
        color: Theme.danger
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }

}
