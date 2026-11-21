import QtQuick
import QtQuick.Controls
import Deadlock.Native

Column {
    id: root
    property var blocks: []
    required property var controller
    property string contextKey
    property bool hidden: true
    spacing: Theme.spacing12

    function escapeText(value) {
        return String(value || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
    }
    function styled(runs) {
        let result = ""
        for (const run of runs || []) {
            let text = escapeText(run.text).replace(/\n/g, "<br>")
            for (const pair of [["bold", "b"], ["italic", "i"], ["underline", "u"], ["strike", "s"]])
                if (run[pair[0]]) text = "<" + pair[1] + ">" + text + "</" + pair[1] + ">"
            if (run.code) text = '<font face="monospace">' + text + '</font>'
            if (/^#[0-9a-f]{6}$/.test(run.color || "")) text = '<font color="' + run.color + '">' + text + '</font>'
            if (/^https?:\/\//.test(run.link || "")) text = '<a href="' + escapeText(run.link) + '">' + text + '</a>'
            result += text
        }
        return result
    }
    Repeater {
        model: root.blocks
        delegate: Loader {
            id: blockLoader
            required property var modelData
            width: root.width
            sourceComponent: modelData.type === "image" ? imageBlock : modelData.type === "rule" ? ruleBlock : modelData.type === "list" ? listBlock : textBlock
            Component {
                id: textBlock
                Item {
                    implicitHeight: body.implicitHeight
                    Rectangle { visible: blockLoader.modelData.type === "quote"; width: 3; height: parent.height; color: Theme.rule }
                    Text {
                        id: body
                        objectName: "richDescriptionText"
                        x: blockLoader.modelData.type === "quote" ? Theme.spacing12 : 0
                        width: parent.width - x
                        text: root.styled(blockLoader.modelData.runs)
                        textFormat: Text.StyledText
                        linkColor: Theme.accentInk
                        wrapMode: Text.Wrap
                        color: Theme.text
                        font.pointSize: blockLoader.modelData.type === "heading" ? Theme.body + (4 - blockLoader.modelData.level) * 2 : Theme.body
                        font.bold: blockLoader.modelData.type === "heading"
                        onLinkActivated: link => root.controller.openLink(link)
                        Accessible.name: text
                    }
                }
            }
            Component {
                id: listBlock
                Column {
                    spacing: Theme.spacing4
                    Repeater {
                        model: blockLoader.modelData.items
                        delegate: Row {
                            required property var modelData
                            required property int index
                            width: blockLoader.width
                            spacing: Theme.spacing8
                            Label { id: marker; text: blockLoader.modelData.ordered ? (index + 1) + "." : "•"; textFormat: Text.PlainText; color: Theme.text }
                            Text {
                                width: parent.width - marker.width - Theme.spacing8
                                text: root.styled(parent.modelData)
                                textFormat: Text.StyledText
                                linkColor: Theme.accentInk
                                font.pointSize: Theme.body
                                wrapMode: Text.Wrap
                                color: Theme.text
                                onLinkActivated: link => root.controller.openLink(link)
                            }
                        }
                    }
                }
            }
            Component {
                id: ruleBlock
                Rectangle { implicitHeight: 1; color: Theme.rule }
            }
            Component {
                id: imageBlock
                ModPreview {
                    source: blockLoader.modelData.url
                    description: blockLoader.modelData.alt || qsTr("Description image")
                    contextKey: root.contextKey
                    hidden: root.hidden
                    gallery: true
                    decodeSize: Qt.size(960, 1080)
                    implicitHeight: ready && imagePaintedSize.width > 0 ? width * imagePaintedSize.height / imagePaintedSize.width : width * (blockLoader.modelData.height || 240) / (blockLoader.modelData.width || 860)
                }
            }
        }
    }
}
