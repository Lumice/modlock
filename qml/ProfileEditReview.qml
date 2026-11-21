pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
ListView {
    id: root
    required property var review
    readonly property var summary: review.summary || ({})
    readonly property var constraint: summary.packageConstraint || ({})
    readonly property var capacity: summary.capacity || ({})
    readonly property bool sharded: summary.version === 2
    readonly property string beforeCapacity: capacityText(capacity.before, review.capacityBeforeBytesText || "")
    readonly property string afterCapacity: capacityText(capacity.after, review.capacityAfterBytesText || "")
    function capacityText(outcome, bytes) {
        if (!outcome) return ""
        const state = outcome.status === "Fits" ? qsTr("Fits") : outcome.status === "ExceedsCapacity" ? qsTr("Too many mods to apply at once") : qsTr("Some mods still need file choices")
        const counts = outcome.counts
        const totals = counts ? qsTr("%1 selected choices · %2 mod packages · %3 companion files\n%4 physical files · %5 bytes to copy")
            .arg(counts.selectedReferences).arg(counts.uniqueFamilies).arg(counts.companionFiles).arg(counts.physicalFiles).arg(bytes) : qsTr("Sizes are shown once every enabled mod has its files chosen.")
        const folders = outcome.shards ? outcome.shards.map(s => qsTr("Folder %1: %2/99").arg(s.index).arg(s.directoryCount)).join(qsTr("; ")) : ""
        const issue = !outcome.issue ? "" : outcome.issue.groupSize == null ? qsTr("%1 (%2): %3").arg(outcome.issue.title).arg(outcome.issue.modId).arg(outcome.issue.reason) : qsTr("%1 (%2): group of %3 unique families. %4").arg(outcome.issue.title).arg(outcome.issue.modId).arg(outcome.issue.groupSize).arg(outcome.issue.reason)
        return state + "\n" + totals + (folders ? "\n" + (outcome.partial ? qsTr("Partial placement only: %1").arg(folders) : folders) : "") + (issue ? "\n" + issue : "")
    }
    visible: !!review.token
    clip: true
    reuseItems: true
    activeFocusOnTab: true
    onVisibleChanged: if (visible) forceActiveFocus()
    model: review.comparisonRows || []
    Accessible.name: qsTr("What will change")
    ScrollBar.vertical: ScrollBar {}
    header: ColumnLayout {
        width: root.width
        spacing: 8
        Label {
            Layout.fillWidth: true
            text: root.summary.canCommit ? ((root.summary.affectedEnabledModIds || []).length === 1 ? qsTr("%1 enabled mod affected. Changes are saved when Deadlock is closed, or when you next play.") : qsTr("%1 enabled mods affected. Changes are saved when Deadlock is closed, or when you next play.")).arg((root.summary.affectedEnabledModIds || []).length) : qsTr("Nothing changed, so there is nothing to save.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: root.sharded
            text: qsTr("Desired capacity before\n%1").arg(root.beforeCapacity)
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        Label {
            Layout.fillWidth: true
            visible: root.sharded
            text: qsTr("Desired capacity after\n%1").arg(root.afterCapacity)
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        Label {
            Layout.fillWidth: true
            visible: root.sharded
            text: qsTr("You can save this profile even if it has too many files. Reduce your selection before playing.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.pointSize: Theme.small
        }
        Label {
            Layout.fillWidth: true
            visible: !root.sharded
            text: qsTr("Package slots before: %1 · after: %2 · limit: 99").arg(root.constraint.selectedBefore == null ? qsTr("Needs selection") : root.constraint.selectedBefore).arg(root.constraint.selectedAfter == null ? qsTr("Needs selection") : root.constraint.selectedAfter)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.pointSize: Theme.small
        }
        Label {
            Layout.fillWidth: true
            visible: !root.sharded && !!root.constraint.afterIssue
            text: root.constraint.afterIssue || ""
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label { text: qsTr("Before → After"); textFormat: Text.PlainText; font.weight: Font.Medium }
    }
    delegate: Label {
        required property var modelData
        width: root.width
        text: modelData.position === undefined ? qsTr("%1 → %2").arg(modelData.before || qsTr("Not selected")).arg(modelData.after || qsTr("Not selected")) : qsTr("%1. %2 → %3\nIDs: %4 → %5").arg(modelData.position).arg(modelData.beforeTitle || modelData.before).arg(modelData.afterTitle || modelData.after).arg(modelData.before).arg(modelData.after)
        textFormat: Text.PlainText
        wrapMode: Text.WrapAnywhere
        padding: 4
    }
}
