pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

ColumnLayout {
    id: root
    required property CatalogController controller
    readonly property var resource: controller.resource
    readonly property bool files: controller.resourceKind === "files"
    readonly property bool comments: controller.resourceKind === "comments"
    readonly property int page: controller.resourcePage
    property bool provenanceExpanded: false
    spacing: 8
    Label { Layout.fillWidth: true; text: root.files ? qsTr("Files (metadata only)") : root.comments ? qsTr("Comments") : qsTr("Changelog"); font.weight: Font.Medium; wrapMode: Text.Wrap }
    Flow {
        Layout.fillWidth: true
        Layout.minimumWidth: 0
        spacing: 6
        Button { text: qsTr("Refresh section"); enabled: !root.controller.busy; onClicked: root.controller.loadResource(root.controller.resourceKind, root.page, true) }
        Button { text: qsTr("Close section"); onClicked: root.controller.closeResource() }
    }
    Label {
        Layout.fillWidth: true
        text: root.resource.state === "hidden" ? qsTr("The provider has hidden comments for this submission.")
            : root.resource.state === "unavailable" ? qsTr("This submission is private or unavailable; cached resources are hidden.")
            : root.resource.state === "missing" ? qsTr("This section is not cached. Refresh the section to fetch it.")
            : !root.resource.items ? qsTr("Loading section…")
            : root.files ? qsTr("File names, sizes and checksums here are cached metadata. Use Download to resolve a fresh list before choosing a file.")
            : (root.resource.items || []).length === 0 ? qsTr("This page has no public entries.") : ""
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        visible: text !== ""
        color: Theme.secondary
    }
    Label {
        Layout.fillWidth: true
        text: {
            const p = root.resource.provenance || {}
            const messages = []
            if (p.stale) messages.push(qsTr("Cached section metadata may be out of date."))
            if (p.incomplete) messages.push(qsTr("Some section metadata is incomplete."))
            const failure = p.refreshError || {}
            if (Object.keys(failure).length) messages.push(qsTr("Last section refresh failed: %1 (%2)").arg(failure.message || "").arg(failure.kind || ""))
            return messages.join("\n")
        }
        visible: text !== ""
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }
    Button {
        visible: !!root.resource.provenance
        text: root.provenanceExpanded ? qsTr("Hide section source details") : qsTr("Section source details")
        onClicked: root.provenanceExpanded = !root.provenanceExpanded
    }
    Label {
        Layout.fillWidth: true
        visible: !!root.resource.provenance && root.provenanceExpanded
        text: {
            const p = root.resource.provenance || {}
            const date = p.fetchedAt === null || p.fetchedAt === undefined ? qsTr("Unknown age") : new Date(Number(p.fetchedAt) * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat)
            return qsTr("%1 • %2%3%4").arg(p.source || "").arg(date).arg(p.stale ? qsTr(" • stale") : "").arg(p.incomplete ? qsTr(" • incomplete") : "")
        }
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        font.pointSize: Theme.small
        color: Theme.secondary
    }
    ListView {
        id: entries
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(contentHeight, 420)
        Layout.minimumHeight: count ? 100 : 0
        clip: true
        reuseItems: true
        model: root.resource.items || []
        spacing: 12
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {}
        delegate: ColumnLayout {
            id: entry
            required property var modelData
            width: entries.width
            spacing: 5
            Label {
                Layout.fillWidth: true
                text: root.files ? entry.modelData.name : root.comments ? (entry.modelData.author || {}).name || qsTr("Unknown author") : entry.modelData.name
                textFormat: Text.PlainText
                font.weight: Font.Medium
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                text: root.files ? qsTr("%1 bytes%2\nMD5: %3").arg(root.controller.displayCount(entry.modelData.size)).arg(entry.modelData.isArchived ? qsTr(" • archived") : "").arg(entry.modelData.md5Checksum || qsTr("Not supplied"))
                    : root.comments ? qsTr("Score %1 • %2 replies%3").arg(entry.modelData.score).arg(entry.modelData.replyCount).arg(entry.modelData.pinned ? qsTr(" • pinned") : "")
                    : entry.modelData.version || ""
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                font.pointSize: Theme.small
                color: Theme.secondary
            }
            Label {
                Layout.fillWidth: true
                visible: entry.modelData.createdAt !== null && entry.modelData.createdAt !== undefined
                text: visible ? new Date(Number(entry.modelData.createdAt) * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat) : ""
                textFormat: Text.PlainText
                color: Theme.secondary
                font.pointSize: Theme.small
            }
            Label {
                Layout.fillWidth: true
                text: root.files ? entry.modelData.description || "" : entry.modelData.text || ""
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: !root.files && !root.comments && (entry.modelData.changes || []).length > 0
                text: (entry.modelData.changes || []).map(change => change.category + ": " + change.text).join("\n")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: entry.modelData.linksTruncated || false
                text: qsTr("Some links could not be displayed.")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            CatalogLinks { Layout.fillWidth: true; controller: root.controller; links: entry.modelData.links || [] }
            Button {
                visible: root.comments && !!(entry.modelData.author || {}).profileUrl
                text: qsTr("Author’s provider profile")
                onClicked: root.controller.openLink(entry.modelData.author.profileUrl)
            }
            Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Theme.rule }
        }
    }
    Flow {
        Layout.fillWidth: true
        Layout.minimumWidth: 0
        spacing: 6
        Button { text: qsTr("Previous section page"); enabled: root.page > 1; onClicked: root.controller.loadResource(root.controller.resourceKind, root.page - 1) }
        Label { padding: 8; text: qsTr("Page %1").arg(Number(root.page).toLocaleString()) }
        Button {
            text: qsTr("Next section page")
            enabled: root.files ? root.page < 7 && root.page * 20 < Number(root.resource.total || 0) : !!root.resource.hasMore && root.page < 500
            onClicked: root.controller.loadResource(root.controller.resourceKind, root.page + 1)
        }
    }
}
