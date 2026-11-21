pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

RowLayout {
    id: root
    required property string text
    property real pointSize: Theme.title
    property string fontFamily: Theme.displayFamily
    property int fontWeight: Font.DemiBold
    property bool uppercase: false
    property real letterSpacing: 0
    property bool showLeading: false
    property alias leadingContent: leadingSlot.data
    default property alias trailing: trailingSlot.data

    Layout.fillWidth: true
    spacing: Theme.spacing12

    Item {
        Layout.preferredWidth: 12
        Layout.preferredHeight: 12
        Layout.alignment: Qt.AlignVCenter
        Accessible.ignored: true
        Rectangle {
            objectName: "sectionHeadingDiamond"
            anchors.centerIn: parent
            width: 7.5
            height: 7.5
            rotation: 45
            color: "transparent"
            border.width: 1.5
            border.color: Theme.brass
            Accessible.ignored: true
        }
    }

    Label {
        objectName: "sectionHeadingLabel"
        Layout.alignment: Qt.AlignVCenter
        Layout.maximumWidth: Math.max(120, root.width - 80)
        text: root.text
        textFormat: Text.PlainText
        font.family: root.fontFamily
        font.pointSize: root.pointSize
        font.weight: root.fontWeight
        font.capitalization: root.uppercase ? Font.AllUppercase : Font.MixedCase
        font.letterSpacing: root.letterSpacing
        elide: Text.ElideRight
    }

    RowLayout {
        id: leadingSlot
        Layout.alignment: Qt.AlignVCenter
        spacing: Theme.spacing8
        visible: root.showLeading
    }

    Rectangle {
        objectName: "sectionHeadingRule"
        Layout.fillWidth: true
        Layout.minimumWidth: 16
        Layout.preferredHeight: 1
        Layout.alignment: Qt.AlignVCenter
        Accessible.ignored: true
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: Theme.ruleStrong }
            GradientStop { position: 1.0; color: "transparent" }
        }
    }

    RowLayout {
        id: trailingSlot
        Layout.alignment: Qt.AlignVCenter
        spacing: Theme.spacing8
        visible: children.length > 0
    }
}
