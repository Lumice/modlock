pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

GridLayout {
    id: root
    required property CatalogController controller
    readonly property var query: controller.queryState
    readonly property bool popupOpen: false
    columns: width < 700 ? 2 : 4
    columnSpacing: 12
    CheckBox { objectName: "browseFavoritesFilter"; Layout.columnSpan: 2; text: qsTr("Favorites only"); checked: root.query.favoritesOnly; onToggled: root.controller.updateQuery({favoritesOnly: checked}) }
    Label { text: qsTr("Author ID") }
    TextField {
        Layout.fillWidth: true
        text: root.query.authorRemoteId || ""
        placeholderText: qsTr("GameBanana author ID")
        Accessible.name: qsTr("Author ID filter")
        onEditingFinished: root.controller.updateQuery({authorRemoteId: text})
    }
    Label { text: qsTr("Find author") }
    TextField {
        id: authorSearch
        Layout.fillWidth: true
        placeholderText: qsTr("Author name")
        Accessible.name: qsTr("Search author names")
        onTextEdited: authorDebounce.restart()
        Timer { id: authorDebounce; interval: 300; onTriggered: root.controller.findAuthors(authorSearch.text) }
    }
    ListView {
        id: authorResults
        Layout.columnSpan: root.columns
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(contentHeight, 130)
        visible: count > 0
        model: root.controller.authors
        reuseItems: true
        clip: true
        delegate: ItemDelegate {
            id: authorEntry
            required property var modelData
            width: authorResults.width
            contentItem: Label { text: qsTr("%1 (%2 submissions)").arg(authorEntry.modelData.name).arg(Number(authorEntry.modelData.submissionCount).toLocaleString()); textFormat: Text.PlainText; wrapMode: Text.Wrap }
            onClicked: root.controller.author(modelData.authorRemoteId)
        }
    }
    CheckBox { text: qsTr("Exclude selected categories/heroes"); checked: root.query.excludeFilters; Layout.columnSpan: 2; onToggled: root.controller.updateQuery({excludeFilters: checked}) }
    CheckBox {
        text: qsTr("Include WIPs")
        enabled: !root.query.submissionType
        checked: root.query.includeWips
        onToggled: root.controller.updateQuery({includeWips: checked})
        ToolTip.visible: hovered || activeFocus
        ToolTip.text: enabled ? qsTr("Include WIPs in this author's submissions.") : qsTr("The selected submission type takes precedence. Include WIPs applies to all author submissions.")
    }
    CheckBox { text: qsTr("Hide obsolete"); checked: root.query.hideObsolete; onToggled: root.controller.updateQuery({hideObsolete: checked}) }
    CheckBox { text: qsTr("Sensitive content only"); checked: root.query.nsfwOnly; Layout.columnSpan: 2; onToggled: { if (checked) Preferences.hideNsfwMods = false; root.controller.updateQuery({nsfwOnly: checked}) } }
    Label { text: qsTr("Updated since") }
    TextField {
        Layout.fillWidth: true
        placeholderText: qsTr("YYYY-MM-DD")
        text: root.query.updatedAfter === null ? "" : new Date(Number(root.query.updatedAfter) * 1000).toISOString().slice(0, 10)
        Accessible.name: qsTr("Updated on or after ISO date")
        onEditingFinished: root.controller.updateQuery({updatedAfter: text})
    }
    Label { text: qsTr("Added since") }
    TextField {
        Layout.fillWidth: true
        placeholderText: qsTr("YYYY-MM-DD")
        text: root.query.addedAfter === null ? "" : new Date(Number(root.query.addedAfter) * 1000).toISOString().slice(0, 10)
        Accessible.name: qsTr("Added on or after ISO date")
        onEditingFinished: root.controller.updateQuery({addedAfter: text})
    }
    Label { text: qsTr("Added before") }
    TextField {
        Layout.fillWidth: true
        placeholderText: qsTr("YYYY-MM-DD")
        text: root.query.addedBefore === null ? "" : new Date(Number(root.query.addedBefore) * 1000).toISOString().slice(0, 10)
        Accessible.name: qsTr("Added before ISO date")
        onEditingFinished: root.controller.updateQuery({addedBefore: text})
    }
    Label {
        Layout.columnSpan: root.columns
        Layout.fillWidth: true
        visible: root.controller.searchPrerequisite !== ""
        text: root.controller.searchPrerequisite
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: Theme.secondary
    }
}
