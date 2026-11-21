pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "HeroData.js" as HeroData

GridView {
    id: root
    property var artwork: ({})
    property string search: ""
    signal heroSelected(string hero)
    objectName: "heroGrid"
    clip: true
    cellWidth: width / Math.max(2, Math.floor(width / 150))
    cellHeight: 142
    model: HeroData.heroes.filter(hero => hero.toLowerCase().includes(search.toLowerCase().trim()))
    ScrollBar.vertical: ScrollBar {}
    delegate: ItemDelegate {
        id: tile
        required property string modelData
        readonly property var skin: root.artwork[modelData] || ({})
        objectName: "heroTile_" + modelData
        width: root.cellWidth - 10
        height: root.cellHeight - 10
        Accessible.name: qsTr("Skins for %1").arg(modelData)
        onClicked: root.heroSelected(modelData)
        contentItem: ColumnLayout {
            spacing: Theme.spacing8
            ModPreview {
                id: heroPreview
                objectName: "heroSkin_" + tile.modelData
                Layout.alignment: Qt.AlignHCenter
                Layout.preferredWidth: tile.width - 20
                Layout.preferredHeight: 64
                radius: Theme.radius4
                source: tile.skin.thumbnail || ""
                hidden: !!source.toString() && tile.skin.masked !== false
                active: root.visible && tile.visible
                contextKey: tile.modelData + ":" + source
                decodeSize: Qt.size(300, 160)
                compact: true
                placeholderTitle: tile.modelData
                description: qsTr("Popular skin for %1").arg(tile.modelData)
                ModPreview {
                    anchors.fill: parent
                    visible: heroPreview.masked
                    source: ""
                    placeholderTitle: tile.modelData
                    active: false
                    Accessible.ignored: true
                }

            }
            Label {
                visible: heroPreview.masked
                Layout.fillWidth: true
                text: qsTr("Preview hidden")
                horizontalAlignment: Text.AlignHCenter
                color: Theme.secondary
                font.pointSize: Theme.small
            }
            Label {
                Layout.fillWidth: true
                text: tile.modelData
                textFormat: Text.PlainText
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                font.weight: Font.DemiBold
            }
        }
    }
    Label {
        anchors.centerIn: parent
        visible: root.count === 0
        text: qsTr("No heroes match your search")
        color: Theme.secondary
    }
}
