pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import Deadlock.Native

ListView {
    id: root
    required property CatalogController controller
    property var links: []
    implicitHeight: Math.min(contentHeight, 150)
    model: links
    clip: true
    reuseItems: true
    boundsBehavior: Flickable.StopAtBounds
    ScrollBar.vertical: ScrollBar {}
    delegate: ItemDelegate {
        id: link
        required property var modelData
        width: root.width
        contentItem: Label { text: (link.modelData.label || link.modelData.platform || link.modelData.url) + (link.modelData.level ? " (" + link.modelData.level + ")" : ""); textFormat: Text.PlainText; wrapMode: Text.Wrap }
        enabled: !!modelData.url
        Accessible.name: modelData.label || modelData.platform || modelData.url || ""
        Accessible.description: modelData.url || ""
        ToolTip {
            id: destinationTip
            visible: link.hovered || link.activeFocus
            text: link.modelData.url || ""
            contentItem: Label { text: destinationTip.text; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere }
        }
        onClicked: root.controller.openLink(modelData.url)
    }
}
