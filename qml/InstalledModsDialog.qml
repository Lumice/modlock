pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// Lists mods that are already in the game folder but not managed by this app,
// with the GameBanana page they most likely come from. Importing copies one
// mod into the library through the normal import review; the game folder is
// only changed later by Apply.
Dialog {
    id: root
    required property LibraryModel library
    property string importError: ""
    signal importRequested(var ids, var linkIds)
    property var selected: ({})
    readonly property var selectedRows: newRows.filter(r => !r.error && selected[r.id])
    function toggle(id, on) { const next = Object.assign({}, selected); if (on) next[id] = true; else delete next[id]; selected = next }
    function selectAll(on) {
        const next = {}
        if (on) for (const r of newRows) if (!r.error && !offInOldManager(r)) next[r.id] = true
        selected = next
    }
    readonly property var rows: library.installedMods
    readonly property var newRows: rows.filter(r => !r.alreadyInLibrary)
    readonly property string scanGame: library.gamePath || library.detectedGamePath
    property string actionError: ""

    // The old manager switched a mod off by renaming its files to "<id>_pakNN_dir.vpk".
    function offInOldManager(row) {
        const paths = (row.packages && row.packages.length) ? row.packages : [row.path]
        return paths.every(p => /^\d+_pak\d+_dir\.vpk$/i.test(fileName(p)))
    }
    function fileName(path) { return path.slice(path.lastIndexOf("/") + 1) }
    function location(path) {
        const at = path.indexOf("/game/citadel/")
        const rel = at >= 0 ? path.slice(at + 14) : path
        return rel.slice(0, rel.lastIndexOf("/") + 1) || rel
    }
    function size(bytes) {
        if (bytes >= 1073741824) return qsTr("%1 GB").arg((bytes / 1073741824).toFixed(1))
        if (bytes >= 1048576) return qsTr("%1 MB").arg((bytes / 1048576).toFixed(1))
        return qsTr("%1 KB").arg(Math.max(1, Math.round(bytes / 1024)))
    }
    function basisText(basis) {
        return basis === "manifest" ? qsTr("from the old Mod Manager's records")
             : basis === "mod manager library" ? qsTr("from your Deadlock Mod Manager library")
             : basis === "exact file hash" ? qsTr("exact file match online")
             : basis
    }
    function startImport(row, link) {
        actionError = ""
        root.importRequested([row.id], link ? [row.id] : [])
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(720, parent ? parent.width - 40 : 720)
    height: Math.min(parent ? parent.height - 40 : 640, 680)
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    title: qsTr("Mods already in the game")

    Component.onCompleted: {
        open()
        if (rows.length === 0 && !library.installedModsBusy && library.canScanInstalledMods && scanGame !== "")
            library.scanInstalledMods()
    }

    footer: DialogButtonBox {
          Button {
            objectName: "installedImportSelected"
            visible: root.selectedRows.length > 0
            highlighted: true
            text: root.selectedRows.length === 1 ? qsTr("Import 1 selected") : qsTr("Import %1 selected").arg(root.selectedRows.length)
            icon.source: Theme.icon("download")
            enabled: root.library.localImport.canPrepare && root.selectedRows.length <= 64
            Accessible.description: qsTr("Review and add all selected mods in one step. Mods with a GameBanana match are linked; you can change that in the review.")
            onClicked: {
                const rows = root.selectedRows
                root.actionError = ""
                root.importRequested(rows.map(r => r.id), rows.filter(r => r.suggestedMatch && r.suggestedMatch.link).map(r => r.id))
            }
          }
        Button { text: qsTr("Close"); onClicked: root.close() }
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: qsTr("Mods in your Deadlock folder that this app doesn't manage yet. Imported mods start switched off; the game doesn't change until you apply.")
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            Label {
                Layout.fillWidth: true
                text: root.scanGame ? root.scanGame : qsTr("Deadlock wasn't found in your Steam libraries. Choose its folder with Apply first.")
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
                color: root.scanGame ? Theme.faint : Theme.warning
                font.pointSize: Theme.small
            }
            BusyIndicator { running: root.library.installedModsBusy; visible: running; Layout.preferredHeight: 24; Layout.preferredWidth: 24 }
            Button {
                objectName: "installedRescan"
                text: root.rows.length ? qsTr("Scan again") : qsTr("Scan")
                icon.source: Theme.icon("refresh")
                enabled: root.library.canScanInstalledMods && !root.library.installedModsBusy && root.scanGame !== ""
                onClicked: { root.actionError = ""; root.library.scanInstalledMods() }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: root.actionError || root.importError || root.library.installedModsError
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.danger
        }
        RowLayout {
          Layout.fillWidth: true
          visible: !root.library.installedModsBusy && root.rows.length > 0
          spacing: Theme.spacing8
          Label {
            Layout.fillWidth: true
            visible: root.rows.length > 0
            text: root.newRows.length === 0 ? qsTr("Everything in the game folder is already in your library.")
                : (root.newRows.length === 1 ? qsTr("1 mod not in your library yet") : qsTr("%1 mods not in your library yet").arg(root.newRows.length))
                  + (root.rows.length > root.newRows.length ? qsTr(" · %1 already imported").arg(root.rows.length - root.newRows.length) : "")
            font.weight: Font.Medium
          }
          Button {
            objectName: "installedSelectAll"
            visible: root.newRows.some(r => !r.error)
            flat: true
            readonly property bool all: root.selectedRows.length > 0 && root.selectedRows.length >= root.newRows.filter(r => !r.error && !root.offInOldManager(r)).length
            text: all ? qsTr("Select none") : qsTr("Select all")
            onClicked: root.selectAll(!all)
          }

        }
        EmptyState {
            Layout.fillWidth: true
            visible: !root.library.installedModsBusy && root.rows.length === 0 && root.library.installedModsError === "" && root.scanGame !== ""
            headline: qsTr("No unmanaged mods found yet.")
            text: qsTr("Press Scan to look again.")
        }
        ListView {
            id: list
            objectName: "installedModsList"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: Theme.spacing8
            model: root.rows.slice().sort((a, b) => Number(a.alreadyInLibrary) - Number(b.alreadyInLibrary))
            ScrollBar.vertical: ScrollBar {}
            delegate: Rectangle {
                id: card
                HoverHandler { id: cardHover }
                required property var modelData
                readonly property var match: modelData.suggestedMatch
                readonly property bool usable: !modelData.alreadyInLibrary && !modelData.error
                width: ListView.view.width - 12
                implicitHeight: cardBody.implicitHeight + Theme.spacing24
                radius: Theme.radius
                color: Theme.surfaceLow
                border.color: Theme.rule
                opacity: modelData.alreadyInLibrary ? 0.65 : 1
                RowLayout {
                    id: cardBody
                    anchors.fill: parent
                    anchors.margins: Theme.spacing12
                    spacing: Theme.spacing12
                    CheckBox {
                        Layout.alignment: Qt.AlignTop
                        visible: card.usable
                        checked: !!root.selected[card.modelData.id]
                        Accessible.name: qsTr("Select %1").arg(card.match && card.match.name ? card.match.name : root.fileName(card.modelData.path))
                        onToggled: root.toggle(card.modelData.id, checked)
                    }
                    Rectangle {
                        Layout.preferredWidth: 72
                        Layout.preferredHeight: 48
                        Layout.alignment: Qt.AlignTop
                        radius: 6
                        color: Theme.surfaceHigh
                        clip: true
                        Image {
                            anchors.fill: parent
                            source: card.match && card.match.thumbnail ? card.match.thumbnail : ""
                            fillMode: Image.PreserveAspectCrop
                            sourceSize: Qt.size(144, 96)
                            asynchronous: true
                            visible: status === Image.Ready
                        }
                        Label {
                            anchors.centerIn: parent
                            visible: !(card.match && card.match.thumbnail)
                            text: "VPK"
                            color: Theme.faint
                            font.pointSize: Theme.small
                            font.weight: Font.DemiBold
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacing8
                            Label {
                                Layout.fillWidth: true
                                text: card.match && card.match.name ? card.match.name
                                    : card.match ? qsTr("GameBanana mod #%1").arg(card.match.modId)
                                    : root.fileName(card.modelData.path)
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                font.weight: Font.DemiBold
                            }
                            StatusPill {
                                visible: card.usable && root.offInOldManager(card.modelData)
                                text: qsTr("Off in the old manager")
                                tone: "muted"
                            }
                            StatusPill {
                                visible: card.modelData.alreadyInLibrary || !!card.modelData.error
                                text: card.modelData.alreadyInLibrary ? qsTr("In library") : qsTr("Can't import")
                                tone: card.modelData.alreadyInLibrary ? "success" : "danger"
                            }
                        }
                        Label {
                            Layout.fillWidth: true
                            text: [card.match && Theme.meaningful(card.match.author) ? qsTr("By %1").arg(card.match.author) : "",
                                   root.size(card.modelData.sizeBytes || 0),
                                   (card.modelData.packages || []).length > 1 ? qsTr("%1 packages").arg(card.modelData.packages.length) : ""].filter(t => t !== "").join(" · ")
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            color: Theme.faint
                            font.pointSize: Theme.small
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: !!card.modelData.error
                            text: card.modelData.error || ""
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            color: Theme.danger
                            font.pointSize: Theme.small
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: card.usable && text !== ""
                            text: !card.match ? qsTr("Not identified yet.")
                                : card.match.basis === "exact file hash" ? qsTr("Identified online by its file fingerprint.") : ""
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            color: Theme.secondary
                            font.pointSize: Theme.small
                        }
                        Flow {
                            Layout.fillWidth: true
                            visible: card.usable
                            spacing: Theme.spacing8
                            Button {
                                visible: !!card.match
                                highlighted: true
                                text: qsTr("Import and link")
                                icon.source: Theme.icon("link")
                                Accessible.description: qsTr("Import this mod and link it to its GameBanana page")
                                enabled: root.library.localImport.canPrepare
                                onClicked: root.startImport(card.modelData, true)
                            }
                            Button {
                                text: card.match ? qsTr("Import without link") : qsTr("Import")
                                flat: !!card.match
                                icon.source: Theme.icon("download")
                                enabled: root.library.localImport.canPrepare
                                onClicked: root.startImport(card.modelData, false)
                            }
                            Button {
                                visible: !card.match
                                flat: true
                                text: qsTr("Identify online")
                                icon.source: Theme.icon("search")
                                Accessible.description: qsTr("Look up this file's fingerprint to find its GameBanana page. Only the file hash is sent.")
                                enabled: !root.library.installedModsBusy
                                onClicked: {
                                    root.actionError = ""
                                    if (!root.library.identifyInstalledMod(card.modelData.id))
                                        root.actionError = qsTr("Couldn't look this mod up right now.")
                                }
                            }
                            Button {
                                visible: !!(card.match && card.match.link)
                                flat: true
                                text: qsTr("Open page")
                                icon.source: Theme.icon("external")
                                onClicked: Qt.openUrlExternally(card.match.link)
                            }
                        }
                    }
                }
            }
        }
    }
}
