pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

// The art area of a Browse sound item: a click-to-play preview with the
// shared one-clip player. Mature or unrated clips need an explicit reveal
// first; a missing URL shows a quiet "No sound preview".
Item {
    id: root
    objectName: "soundPreview"
    required property string identity
    required property string heading
    property string audioUrl: ""
    // Hidden or unknown content rating without the mature-preview preference.
    property bool concealed: true
    property bool interactive: false
    property bool compact: false
    property real radius: 0
    // Height of the area above a poster title; controls stay inside it.
    property real controlsHeight: height
    readonly property var player: typeof audioPreview !== "undefined" ? audioPreview : null
    readonly property bool hasAudio: audioUrl !== "" && !!player
    readonly property bool current: !!player && identity !== "" && player.currentIdentity === identity
    readonly property bool revealed: !!player && player.revealedIdentities.indexOf(identity) >= 0
    readonly property bool needsReveal: hasAudio && concealed && !revealed
    readonly property string phase: current ? player.state : "idle"
    readonly property bool playing: phase === "playing"
    readonly property bool loading: phase === "loading"
    readonly property string note: !hasAudio ? qsTr("No sound preview")
                                 : needsReveal ? qsTr("Mature sound")
                                 : phase === "error" ? player.message : ""

    Rectangle {
        anchors.fill: parent
        radius: root.radius
        gradient: Gradient {
            GradientStop { position: 0; color: Qt.alpha(Theme.accent, Tokens.dark ? .20 : .14) }
            GradientStop { position: 1; color: Theme.surfaceLow }
        }
    }
    ColorImage {
        // Decorative mark; the controls carry the accessible names.
        Accessible.ignored: true
        visible: !root.compact
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Theme.spacing8
        width: 28; height: 28
        source: Theme.icon("sound"); sourceSize: Qt.size(28, 28)
        color: Theme.faint
        opacity: .7
    }
    ColumnLayout {
        id: controls
        x: Theme.spacing4
        width: parent.width - 2 * Theme.spacing4
        y: Math.max(0, (Math.min(root.height, root.controlsHeight) - implicitHeight) / 2)
        spacing: root.compact ? 2 : Theme.spacing8
        ColorImage {
            Accessible.ignored: true
            visible: !root.hasAudio
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: root.compact ? 20 : 28; Layout.preferredHeight: Layout.preferredWidth
            source: Theme.icon("sound"); sourceSize: Qt.size(28, 28)
            color: Theme.faint
        }
        Button {
            id: revealButton
            objectName: "soundRevealButton"
            visible: root.needsReveal
            enabled: root.interactive
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: controls.width
            text: qsTr("Reveal")
            icon.source: Theme.icon("sound")
            icon.color: Theme.text
            display: root.compact ? AbstractButton.TextOnly : AbstractButton.TextBesideIcon
            Accessible.name: qsTr("Reveal mature sound preview of %1").arg(root.heading)
            ToolTip.visible: (hovered || visualFocus) && root.compact
            ToolTip.text: qsTr("Mature sound. Reveal to play it.")
            onClicked: if (root.interactive && root.player) root.player.reveal(root.identity)
        }
        Button {
            id: playButton
            objectName: "soundPlayButton"
            visible: root.hasAudio && !root.needsReveal
            enabled: root.interactive
            Layout.alignment: Qt.AlignHCenter
            readonly property int size: root.compact ? 34 : 44
            readonly property int iconSize: root.compact ? 16 : 20
            Layout.preferredWidth: size; Layout.preferredHeight: size
            implicitWidth: size; implicitHeight: size
            padding: 0; leftPadding: 0; rightPadding: 0
            display: AbstractButton.IconOnly
            icon.source: Theme.icon(root.loading ? "stop" : root.playing ? "pause" : "play")
            icon.color: Theme.text
            icon.width: iconSize; icon.height: iconSize
            text: root.loading ? qsTr("Stop loading") : root.playing ? qsTr("Pause") : qsTr("Play")
            Accessible.name: (root.loading ? qsTr("Stop loading sound preview of %1")
                              : root.playing ? qsTr("Pause sound preview of %1")
                              : root.phase === "error" ? qsTr("Try sound preview of %1 again")
                              : qsTr("Play sound preview of %1")).arg(root.heading)
            Accessible.description: root.note
            background: Rectangle {
                radius: width / 2
                color: playButton.down ? Theme.accentSoft : playButton.hovered ? Theme.surfaceHover : Qt.alpha(Theme.surface, .92)
                border.width: playButton.visualFocus ? 2 : 1
                border.color: playButton.visualFocus ? Theme.focus : root.current && root.phase !== "error" ? Theme.accentRing : Theme.ruleStrong
            }
            BusyIndicator {
                anchors.centerIn: parent
                width: parent.width + 8; height: width
                visible: root.loading && !Tokens.reduceMotion
                running: visible
                Accessible.ignored: true
            }
            ToolTip.visible: (hovered || visualFocus) && root.compact && root.note !== ""
            ToolTip.text: root.note
            onClicked: if (root.interactive && root.player) root.player.toggle(root.identity, root.audioUrl, root.concealed)
        }
        Label {
            objectName: "soundNote"
            visible: root.note !== "" && (!(root.compact && root.hasAudio) || root.phase === "error")
            Layout.fillWidth: true
            Layout.maximumWidth: controls.width
            horizontalAlignment: Text.AlignHCenter
            text: root.compact && root.phase === "error" ? qsTr("Can't play") : root.note
            textFormat: Text.PlainText
            color: root.phase === "error" ? Theme.warning : Theme.secondary
            font.pointSize: Theme.small
            wrapMode: Text.Wrap
            maximumLineCount: root.compact ? 2 : 3
            elide: Text.ElideRight
        }
    }
}
