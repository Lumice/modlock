pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

ColumnLayout {
    id: root
    required property CatalogController controller
    property bool demoMode: false
    property bool headerActions: false
    readonly property bool hasNotice: !!(controller.prerequisite || controller.busy || friendlyError || notices
                                         || controller.workNotice || controller.contextNotice || (provenanceExpanded && diagnostics))
    visible: !headerActions || hasNotice
    property bool compactProvenance: false
    property bool provenanceExpanded: false
    readonly property var provenance: controller.provenance || ({})
    readonly property bool hasSavedResults: controller.model.count > 0 || !!(controller.detail.summary || {}).identity
    readonly property string notices: {
        const messages = []
        if (!hasSavedResults) return ""
        if (Object.keys(provenance.refreshError || {}).length) messages.push(qsTr("Showing saved results."))
        else if (provenance.stale) messages.push(qsTr("Showing saved results; they may be out of date."))
        return messages.join(" ")
    }
    // Plain wording for the status line; the raw provider text stays in Details.
    readonly property string friendlyError: {
        const e = controller.error || ""
        if (!e) return ""
        if (e.includes("catalogRateLimited")) return qsTr("GameBanana asked us to slow down; try again in a minute.")
        if (e.includes("catalogTransport") || e.includes("catalogUnavailable")) return qsTr("Couldn't reach GameBanana.")
        if (e.includes("catalogIncomplete")) return qsTr("Only part of the catalog could be loaded.")
        if (e.includes("catalogCancelled")) return qsTr("Refresh cancelled.")
        if (e.includes("catalogFull") || e.includes("catalogStorage")) return qsTr("The catalog cache is full or can't be written.")
        return qsTr("Couldn't refresh the catalog.")
    }
    readonly property string diagnostics: {
        const lines = []
        if (controller.errorSummary && controller.errorSummary !== friendlyError) lines.push(Theme.friendlyError(controller.errorSummary))
        if (controller.provenanceText) lines.push(controller.provenanceText)
        if (controller.prerequisite) lines.push(demoMode ? qsTr("Browsing needs a real library and is not available in the demo.") : controller.prerequisite)
        return lines.join("\n\n")
    }
    // Progress text from the helper ("Loading catalog: 2300 of 5551 loaded") while a refresh runs.
    readonly property bool hasProgress: Object.keys(controller.syncProgress || {}).length > 0
    readonly property string busyText: hasProgress ? controller.refreshMessage : qsTr("Updating catalog…")
    spacing: Theme.spacing4
    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacing8
        BusyIndicator {
            Layout.preferredWidth: 20
            Layout.preferredHeight: 20
            visible: root.controller.busy
            running: visible && root.visible
            Accessible.name: qsTr("Catalog work in progress")
        }
        Label {
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            text: [root.controller.prerequisite ? (root.demoMode ? qsTr("Browsing needs a real library and is not available in the demo.") : root.controller.prerequisite) : (root.controller.busy ? root.busyText : ""),
                   root.friendlyError,
                   root.notices && !root.friendlyError.includes(root.notices) && !root.busyText.includes(root.notices) ? root.notices : ""].filter(part => !!part).filter((part, index, parts) => parts.indexOf(part) === index).join(" · ")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
            font.pointSize: Theme.small
        }
        Button { visible: !root.headerActions; text: qsTr("Refresh"); flat: true; icon.source: Theme.icon("refresh"); enabled: root.controller.available && !root.controller.busy; onClicked: root.controller.refresh() }
        Button { text: qsTr("Cancel"); flat: true; visible: root.controller.busy; onClicked: root.controller.cancel() }
        Button { text: qsTr("Retry"); flat: true; visible: root.controller.error !== ""; enabled: root.controller.available && !root.controller.busy; onClicked: root.controller.retry() }
        Button {
            visible: !root.headerActions && root.diagnostics !== ""
            flat: true
            text: root.provenanceExpanded ? qsTr("Hide details") : qsTr("Details")
            onClicked: root.provenanceExpanded = !root.provenanceExpanded
        }
    }
    Label {
        Layout.fillWidth: true
        visible: root.controller.workNotice !== ""
        text: root.controller.workNotice
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        font.pointSize: Theme.small
    }
    Label {
        Layout.fillWidth: true
        visible: root.controller.contextNotice !== ""
        text: root.controller.contextNotice
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        font.pointSize: Theme.small
        color: Theme.secondary
    }
    Label {
        Layout.fillWidth: true
        visible: root.provenanceExpanded && root.diagnostics !== ""
        text: root.diagnostics
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        font.pointSize: Theme.small
        color: Theme.secondary
    }
}
