pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// Shows exactly which lines of a game config file change before anything is
// written. Used for autoexec and crosshair saves (and their undo).
Dialog {
    id: root
    required property GameConfigController controller
    property string heading
    readonly property var review: controller.review || ({})
    readonly property var diffs: [review.diff ? Object.assign({ path: review.path }, review.diff) : null, review.additionalDiff].filter(d => !!d)
    property bool committing: false

    function fileName(path) { return (path || "").slice((path || "").lastIndexOf("/") + 1) }
    // Line diff (LCS) with unchanged runs collapsed to 2 lines of context.
    function diffLines(before, after) {
        const a = before === null || before === undefined ? [] : before.replace(/\r\n/g, "\n").split("\n")
        const b = after === null || after === undefined ? [] : after.replace(/\r\n/g, "\n").split("\n")
        const n = a.length, m = b.length
        if (n * m > 4000000) return [{ kind: "note", text: qsTr("The file is too large to compare line by line.") }]
        const dp = []
        for (let i = 0; i <= n; ++i) dp.push(new Int32Array(m + 1))
        for (let i = n - 1; i >= 0; --i)
            for (let j = m - 1; j >= 0; --j)
                dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
        const out = []
        let i = 0, j = 0
        while (i < n || j < m) {
            if (i < n && j < m && a[i] === b[j]) { out.push({ kind: "same", text: a[i] }); ++i; ++j }
            else if (i < n && (j >= m || dp[i + 1][j] >= dp[i][j + 1])) { out.push({ kind: "remove", text: a[i] }); ++i }
            else { out.push({ kind: "add", text: b[j] }); ++j }
        }
        const keep = out.map((l, k) => l.kind !== "same" || out.slice(Math.max(0, k - 2), k + 3).some(x => x.kind !== "same"))
        const shown = []
        let skipped = 0
        for (let k = 0; k < out.length; ++k) {
            if (keep[k]) {
                if (skipped > 0) { shown.push({ kind: "note", text: skipped === 1 ? qsTr("1 unchanged line") : qsTr("%1 unchanged lines").arg(skipped) }); skipped = 0 }
                shown.push(out[k])
            } else ++skipped
        }
        if (skipped > 0) shown.push({ kind: "note", text: skipped === 1 ? qsTr("1 unchanged line") : qsTr("%1 unchanged lines").arg(skipped) })
        return shown
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(820, parent ? parent.width - 40 : 820)
    height: Math.min(parent ? parent.height - 40 : 640, 680)
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    title: review.undo ? qsTr("Undo last save") : root.heading
    Connections {
        target: root.controller
        function onChanged() {
            // A finished commit clears the review; close once the write is done.
            if (root.committing && !root.controller.busy && !root.controller.review.token) {
                root.committing = false
                if (!root.controller.error) root.close()
            }
        }
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: root.review.undo ? qsTr("These files go back to how they were before your last save. Close Deadlock first.")
                                   : qsTr("These lines change in the game's config files. A backup is kept so you can undo. Close Deadlock first.")
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Label {
            visible: !root.review.token && root.controller.error === ""
            text: qsTr("Preparing the review…")
            color: Theme.secondary
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                width: parent.width
                spacing: Theme.spacing12
                Repeater {
                    model: root.diffs
                    ColumnLayout {
                        id: file
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 4
                        Label {
                            text: file.modelData.before === null || file.modelData.before === undefined ? qsTr("%1 (new file)").arg(root.fileName(file.modelData.path))
                                : file.modelData.after === null || file.modelData.after === undefined ? qsTr("%1 (removed)").arg(root.fileName(file.modelData.path))
                                : root.fileName(file.modelData.path)
                            font.weight: Font.DemiBold
                            textFormat: Text.PlainText
                        }
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: lines.implicitHeight + 12
                            radius: Theme.radius
                            color: Theme.surfaceLow
                            border.color: Theme.rule
                            Column {
                                id: lines
                                x: 6; y: 6
                                width: parent.width - 12
                                Repeater {
                                    model: root.diffLines(file.modelData.before, file.modelData.after)
                                    Rectangle {
                                        id: line
                                        required property var modelData
                                        width: lines.width
                                        height: lineText.implicitHeight + 2
                                        color: modelData.kind === "add" ? Theme.successSoft : modelData.kind === "remove" ? Theme.dangerSoft : "transparent"
                                        Label {
                                            id: lineText
                                            x: 4
                                            width: parent.width - 8
                                            text: (line.modelData.kind === "add" ? "+ " : line.modelData.kind === "remove" ? "− " : line.modelData.kind === "note" ? "" : "  ") + line.modelData.text
                                            textFormat: Text.PlainText
                                            wrapMode: Text.WrapAnywhere
                                            font.family: line.modelData.kind === "note" ? Qt.application.font.family : "monospace"
                                            font.pointSize: Theme.small
                                            font.italic: line.modelData.kind === "note"
                                            color: line.modelData.kind === "note" ? Theme.faint : Theme.text
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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
        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            Button { text: qsTr("Cancel"); onClicked: root.close() }
            Button {
                objectName: "configCommit"
                highlighted: true
                text: root.review.undo ? qsTr("Undo") : qsTr("Save to game")
                icon.source: Theme.icon("check")
                enabled: !!root.review.token && !root.controller.busy
                onClicked: { root.committing = true; if (!root.controller.commit(root.review.token)) root.committing = false }
            }
        }
    }
}
