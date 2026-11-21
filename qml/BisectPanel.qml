pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native

// Library banner while "Find a broken mod" runs: what to do this round,
// the three answers, and the result at the end.
Rectangle {
    id: root
    required property LibraryModel library
    readonly property var session: library.bisect || ({})
    readonly property bool active: !!session.active
    readonly property bool done: !!session.done
    readonly property bool changed: !!session.profileChanged
    readonly property var culprits: (session.result || []).map(id => title(id))
    readonly property var testing: (session.testedMods || []).map(id => title(id))
    property bool showTesting: false
    function title(id) { const m = library.modDetails(id); return (m && m.title) || id }

    visible: active
    Layout.fillWidth: true
    implicitHeight: column.implicitHeight + Theme.spacing24
    radius: Theme.radius
    color: done ? Theme.successSoft : changed ? Theme.warningSoft : Theme.accentSoft
    border.color: done ? Qt.alpha(Theme.success, 0.4) : changed ? Qt.alpha(Theme.warning, 0.4) : Qt.alpha(Theme.accent, 0.35)

    ColumnLayout {
        id: column
        anchors { left: parent.left; right: parent.right; top: parent.top; margins: Theme.spacing12 }
        spacing: Theme.spacing8
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            ColorImage {
                source: Theme.icon(root.done ? "check" : root.changed ? "alert" : "search")
                sourceSize: Qt.size(18, 18)
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                color: root.done ? Theme.success : root.changed ? Theme.warning : Theme.accentInk
            }
            Label {
                Layout.fillWidth: true
                font.weight: Font.DemiBold
                text: root.done ? (root.culprits.length ? qsTr("Found it") : qsTr("Search finished"))
                    : qsTr("Finding a broken mod · Round %1").arg(Number(root.session.round || 1))
                textFormat: Text.PlainText
            }
            Button {
                visible: !root.done
                flat: true
                text: qsTr("Stop and restore")
                enabled: !root.library.busy
                onClicked: root.library.finishBisect(false)
            }
        }

        Label {
            visible: !!root.session.finishNotice
            Layout.fillWidth: true
            text: root.session.finishNotice || ""
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }

        // In progress
        Label {
            Layout.fillWidth: true
            visible: !root.done && !root.changed
            wrapMode: Text.Wrap
            text: (root.session.suspectsCount === 1 ? qsTr("%1 mod could be the cause. About %2 more launches.") : qsTr("%1 mods could be the cause. About %2 more launches.")).arg(Number(root.session.suspectsCount || 0)).arg(Number(root.session.expectedRemainingLaunches || 0))
                + "\n" + qsTr("Press Play, check whether the problem is still there, close the game, then answer below.")
            color: Theme.secondary
        }
        RowLayout {
            Layout.fillWidth: true
            visible: !root.done && !root.changed
            spacing: Theme.spacing8
            Button {
                objectName: "bisectBroken"
                highlighted: true
                text: qsTr("Still broken")
                enabled: !root.library.busy
                onClicked: root.library.answerBisect("broken")
            }
            Button {
                objectName: "bisectFine"
                text: qsTr("Works now")
                enabled: !root.library.busy
                onClicked: root.library.answerBisect("fine")
            }
            Button {
                text: qsTr("Not sure")
                flat: true
                enabled: !root.library.busy
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Use this if you couldn't check. It counts as working, so the search keeps going.")
                onClicked: root.library.answerBisect("unsure")
            }
            Item { Layout.fillWidth: true }
            Button {
                flat: true
                text: root.showTesting ? qsTr("Hide mods on this round") : qsTr("Mods on this round (%1)").arg(root.testing.length)
                onClicked: root.showTesting = !root.showTesting
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.showTesting && !root.done && !root.changed
            text: root.testing.length ? root.testing.join(" · ") : qsTr("None: this round tests the game without these mods.")
            wrapMode: Text.Wrap
            textFormat: Text.PlainText
            color: Theme.secondary
            font.pointSize: Theme.small
        }

        // Profile edited mid-search
        Label {
            Layout.fillWidth: true
            visible: root.changed
            wrapMode: Text.Wrap
            text: root.done ? qsTr("You changed this profile after the result. Finishing keeps those changes; keeping the problem mods off only disables those mods.") : qsTr("You changed this profile during the search, so the answers no longer match. Stop to put your mods back the way they were.")
        }

        // Result
        Label {
            Layout.fillWidth: true
            visible: root.done
            wrapMode: Text.Wrap
            textFormat: Text.PlainText
            text: root.culprits.length === 1 ? qsTr("The problem comes from: %1").arg(root.culprits[0])
                : root.culprits.length > 1 ? qsTr("These mods only break together: %1").arg(root.culprits.join(", "))
                : qsTr("The problem doesn't seem to come from the mods you tested.")
        }
        Label {
            Layout.fillWidth: true
            visible: root.done
            wrapMode: Text.Wrap
            color: Theme.secondary
            text: root.changed ? qsTr("Your current profile changes will be kept. Check the mod's page for an update, or remove it.") : qsTr("Your other mods come back exactly as they were. Check the mod's page for an update, or remove it.")
        }
        RowLayout {
            Layout.fillWidth: true
            visible: root.done
            spacing: Theme.spacing8
            Button {
                objectName: "bisectKeepOff"
                highlighted: true
                visible: root.culprits.length > 0
                text: root.culprits.length > 1 ? qsTr("Restore, keep these off") : qsTr("Restore, keep it off")
                enabled: !root.library.busy
                onClicked: root.library.finishBisect(true)
            }
            Button {
                text: root.culprits.length > 0 ? qsTr("Restore everything") : qsTr("Restore my mods")
                enabled: !root.library.busy
                onClicked: root.library.finishBisect(false)
            }
        }
    }
}
