pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

Button {
    id: root
    required property string title
    property string subtitle: ""
    property bool showSubtitle: subtitle.length > 0
    property url imageSource: ""
    property bool masked: false
    property bool previewActive: true
    // Poster look (Recently added only): the art fills the tile and the title sits over a scrim.
    // Only the art inside the tile zooms; the button and its ancestors never move, because
    // Home admits previews from their untransformed layout positions.
    property bool poster: false
    required property string contextKey
    property int enterIndex: -1
    property bool enterArmed: false
    property int enterGeneration: 0
    property real enterOpacity: 1
    property real enterOffsetY: 0
    property int lastEnterGeneration: 0
    opacity: enterOpacity

    function snapEnter() {
        enterAnim.stop()
        enterOpacity = 1
        enterOffsetY = 0
    }
    function tryEnter() {
        if (!enterArmed || enterGeneration <= 0 || lastEnterGeneration === enterGeneration) return
        lastEnterGeneration = enterGeneration
        if (enterIndex >= 0 && enterIndex < 12 && !Tokens.reduceMotion && Tokens.normal > 0 && visible) {
            enterOpacity = 0
            enterOffsetY = 6
            enterPause.duration = enterIndex * 25
            enterAnim.restart()
        } else {
            snapEnter()
        }
    }
    onEnterArmedChanged: tryEnter()
    onVisibleChanged: if (!visible) snapEnter()
    Component.onCompleted: tryEnter()
    SequentialAnimation {
        id: enterAnim
        PauseAnimation { id: enterPause; duration: 0 }
        ParallelAnimation {
            NumberAnimation { target: root; property: "enterOffsetY"; from: 6; to: 0; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutCubic }
            NumberAnimation { target: root; property: "enterOpacity"; from: 0; to: 1; duration: Math.max(1, Tokens.normal); easing.type: Easing.OutCubic }
        }
    }
    Connections {
        target: Tokens
        function onReduceMotionChanged() { if (Tokens.reduceMotion) root.snapEnter() }
    }
    Translate { id: tileEnterShift; y: root.enterOffsetY }

    padding: 0
    horizontalPadding: 0
    verticalPadding: 0
    // The style's side padding stays on the shelf tiles; a poster's art runs edge to edge.
    leftPadding: root.poster ? 0 : Tokens.controlPadding
    rightPadding: root.poster ? 0 : Tokens.controlPadding
    hoverEnabled: true
    Accessible.role: Accessible.Button
    Accessible.name: root.showSubtitle ? root.title + ", " + root.subtitle : root.title
    ToolTip.visible: (hovered || visualFocus) && (root.poster ? posterTitle.truncated : titleLabel.truncated || (subtitleLabel.visible && subtitleLabel.truncated))
    ToolTip.text: root.showSubtitle ? root.title + "\n" + root.subtitle : root.title

    TextMetrics {
        id: titleMetrics
        font.pointSize: Theme.body
        font.weight: Font.DemiBold
        text: "Ag"
    }

    background: Rectangle {
        transform: tileEnterShift
        radius: Theme.radiusCard
        color: root.down ? Theme.surfacePressed : root.hovered ? Theme.surfaceHover : Theme.surface
        border.width: root.visualFocus ? 2 : 1
        border.color: root.visualFocus ? Theme.accentInk : root.hovered ? Theme.ruleStrong : Theme.rule
        Behavior on color { ColorAnimation { duration: Theme.motionFast } }
    }
    // Poster corners and edge: clip cannot round, so the page colour is painted outside a
    // radiusCard outline, then the edge (brass on hover) is drawn over the art.
    Rectangle {
        z: 2
        transform: tileEnterShift
        visible: root.poster
        anchors.fill: parent
        anchors.margins: -5
        radius: Theme.radiusCard + 5
        color: "transparent"
        border.width: 5
        border.color: Theme.window
    }
    Rectangle {
        z: 2
        transform: tileEnterShift
        visible: root.poster
        anchors.fill: parent
        radius: Theme.radiusCard
        color: "transparent"
        border.width: root.visualFocus ? 2 : 1
        border.color: root.visualFocus ? Theme.accentInk : root.hovered ? Qt.alpha(Theme.brass, .6) : Theme.rule
        Behavior on border.color { ColorAnimation { duration: Theme.motionFast } }
    }

    contentItem: ColumnLayout {
        transform: tileEnterShift
        spacing: 0
        Item {
            id: previewFrame
            Layout.fillWidth: true
            Layout.preferredHeight: root.poster ? Math.round(Math.max(120, width))
                                                : Math.max(76, Math.round(Math.max(120, width) * 9 / 16))
            clip: true
            ModPreview {
                id: homePreview
                anchors.fill: parent
                anchors.margins: root.poster ? 0 : 1
                artworkScale: root.poster && root.hovered && homePreview.ready && !Tokens.reduceMotion ? 1.05 : 1
                Behavior on artworkScale { NumberAnimation { duration: Theme.motionNormal } }
                source: root.imageSource
                hidden: root.masked
                active: root.previewActive
                placeholderTitle: root.title
                contextKey: root.contextKey
                compact: true
                decodeSize: Qt.size(480, 270)
                radius: root.poster ? 0 : Theme.radius4
            }
            Rectangle {
                visible: root.poster
                anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                height: parent.height * .62
                gradient: Gradient {
                    GradientStop { position: 0; color: Qt.alpha(Theme.surface, 0) }
                    GradientStop { position: .45; color: Qt.alpha(Theme.surface, .72) }
                    GradientStop { position: 1; color: Qt.alpha(Theme.surface, .97) }
                }
            }
            Label {
                id: posterTitle
                visible: root.poster
                anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                anchors.leftMargin: Theme.spacing12; anchors.rightMargin: Theme.spacing12; anchors.bottomMargin: 10
                text: root.title
                textFormat: Text.PlainText
                font.family: Theme.displayFamily !== "" ? Theme.displayFamily : titleMetrics.font.family
                font.pointSize: Theme.title
                font.weight: Font.DemiBold
                lineHeight: .95
                color: Theme.text
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                Accessible.ignored: true
            }
        }
        ColumnLayout {
            visible: !root.poster
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: Theme.spacing12
            spacing: Theme.spacing4
            Label {
                id: titleLabel
                Layout.fillWidth: true
                Layout.minimumHeight: Math.ceil(titleMetrics.height * 2)
                text: root.title
                textFormat: Text.PlainText
                font.pointSize: Theme.body
                font.weight: Font.DemiBold
                color: Theme.text
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                verticalAlignment: Text.AlignTop
            }
            Label {
                id: subtitleLabel
                visible: root.showSubtitle
                Layout.fillWidth: true
                text: root.subtitle
                textFormat: Text.PlainText
                font.pointSize: Theme.small
                color: Theme.secondary
                maximumLineCount: 1
                elide: Text.ElideRight
            }
        }
    }
}
