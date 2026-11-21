pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// One review for several mods found in the game folder. Everything is added
// in a single step (all or nothing), switched off.
Dialog {
    id: root
    required property LocalImportController controller
    signal hidden()
    readonly property var rows: (controller.review.groupRows && controller.review.groupRows.length ? controller.review.groupRows : controller.groupRows) || []
    readonly property bool done: controller.phase === "succeeded"
    property var originalLinks: ({})
    property string reviewedGeneration: ""

    function rowBytes(row) {
        if (typeof row.bytes === "number" && row.bytes > 0) return row.bytes
        const draft = (controller.groupRows || []).find(r => r.id === row.id)
        if (draft && Number(draft.bytes) > 0) return Number(draft.bytes)
        return (row.files || []).reduce((sum, f) => sum + (f.members || []).reduce((s, m) => s + (Number(m.size) || 0), 0), 0)
    }
    function fileCount(row) {
        return (row.files || []).reduce((sum, f) => sum + Math.max(1, (f.members || []).length), 0)
    }
    function totalBytes() { return rows.reduce((sum, r) => sum + rowBytes(r), 0) }
    // Edits invalidate the review; ask for a fresh one once per draft.
    function ensureReview() {
        if (done || controller.review.token || !controller.draftCurrent || controller.busy || !controller.canReview) return
        if (reviewedGeneration === controller.draftGeneration) return
        reviewedGeneration = controller.draftGeneration
        if (!controller.reviewCurrentDraft()) reviewedGeneration = ""
    }
    Connections { target: root.controller; function onChanged() { reviewTimer.restart() } }
    Timer { id: reviewTimer; interval: 250; onTriggered: root.ensureReview() }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(760, parent ? parent.width - 40 : 760)
    height: Math.min(parent ? parent.height - 40 : 680, 700)
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    closePolicy: Popup.CloseOnEscape
    title: done ? qsTr("Mods added") : rows.length === 1 ? qsTr("Import 1 mod") : qsTr("Import %1 mods").arg(rows.length)
    Component.onCompleted: {
        const links = {}
        for (const r of controller.groupRows || []) links[r.id] = (r.metadata || {}).link || ""
        originalLinks = links
        open()
        reviewTimer.start()
    }
    onClosed: hidden()

    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: root.done
                ? (root.controller.groupResult.added || []).length === 1 ? qsTr("1 mod was added to your library, switched off. Turn mods on in the library when you're ready.")
                  : qsTr("%1 mods were added to your library, switched off. Turn mods on in the library when you're ready.").arg((root.controller.groupResult.added || []).length)
                : qsTr("These mods are copied into your library in one step: either all of them are added, or none. They start switched off, and nothing in the game changes until you apply.")
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        ListView {
            id: list
            objectName: "groupImportList"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: Theme.spacing8
            model: root.rows
            ScrollBar.vertical: ScrollBar {}
            delegate: Rectangle {
                id: card
                required property var modelData
                readonly property var meta: modelData.metadata || {}
                readonly property string original: root.originalLinks[modelData.id] || ""
                width: ListView.view.width - 12
                implicitHeight: body.implicitHeight + Theme.spacing24
                radius: Theme.radius
                color: Theme.surfaceLow
                border.color: modelData.problem ? Theme.danger : Theme.rule
                ColumnLayout {
                    id: body
                    anchors.fill: parent
                    anchors.margins: Theme.spacing12
                    spacing: 4
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacing8
                        Label {
                            Layout.fillWidth: true
                            text: card.meta.name || qsTr("Unnamed mod")
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            font.weight: Font.DemiBold
                        }
                        Button {
                            visible: !root.done && root.rows.length > 1
                            flat: true
                            text: qsTr("Leave out")
                            icon.source: Theme.icon("x")
                            enabled: !root.controller.busy
                            onClicked: root.controller.dropGroupRow(card.modelData.id)
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        text: [card.meta.author && card.meta.author !== "Unknown" ? qsTr("By %1").arg(card.meta.author) : "",
                               card.meta.category || "",
                               root.fileCount(card.modelData) === 1 ? qsTr("1 file") : qsTr("%1 files").arg(root.fileCount(card.modelData)),
                               Theme.bytes(root.rowBytes(card.modelData))].filter(s => s !== "").join(" · ")
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        color: Theme.faint
                        font.pointSize: Theme.small
                    }
                    CheckBox {
                        visible: card.original !== "" || card.meta.link
                        enabled: !root.done && !root.controller.busy
                        checked: !!card.meta.link
                        text: qsTr("Link to its GameBanana page")
                        onToggled: root.controller.setGroupLink(card.modelData.id, checked ? card.original : "")
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: !!card.modelData.problem
                        text: (card.modelData.problem || {}).message || ""
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.danger
                        font.pointSize: Theme.small
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: !root.done
            text: root.controller.busy && root.controller.phase !== "ready"
                    ? qsTr("Checking files… %1 of %2").arg(Theme.bytes(root.controller.progress.bytesDone)).arg(Theme.bytes(root.controller.progress.bytesTotal))
                : root.controller.phase === "committing" ? qsTr("Adding to library…")
                : root.controller.canCommit ? qsTr("Ready: %1 to copy.").arg(Theme.bytes(root.totalBytes()))
                : root.rows.some(r => !!r.problem) ? qsTr("Leave out the mods with problems to continue.")
                : qsTr("Preparing the review…")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Label {
            Layout.fillWidth: true
            visible: root.controller.error !== ""
            text: root.controller.error
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.danger
        }
        RowLayout {
            Layout.fillWidth: true
            Button {
                objectName: "groupCancel"
                visible: !root.done
                text: qsTr("Cancel import")
                enabled: root.controller.canCancel
                onClicked: { root.controller.cancel(); root.close() }
            }
            Item { Layout.fillWidth: true }
            Button { text: root.done ? qsTr("Close") : qsTr("Later"); onClicked: root.close() }
            Button {
                objectName: "groupCommit"
                visible: !root.done
                highlighted: true
                text: root.rows.length === 1 ? qsTr("Add 1 mod") : qsTr("Add %1 mods").arg(root.rows.length)
                icon.source: Theme.icon("check")
                enabled: root.controller.canCommit
                onClicked: root.controller.commit()
            }
        }
    }
}
