pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "LaunchPresentation.js" as LaunchPresentation

ScrollView {
    id: root
    required property LaunchController controller
    required property var profiles
    required property bool installedFonts
    readonly property var ready: controller.review.summary ? controller.review : controller.attempt.ready || ({})
    readonly property var summary: ready.summary || ({})
    readonly property var identity: controller.review.summary || !ready.summary
        ? ({mode: controller.mode, profileName: controller.targetProfileName})
        : LaunchPresentation.identity(ready, profiles)
    readonly property bool changed: ready.changed === true
    clip: true
    contentWidth: availableWidth
    activeFocusOnTab: true
    Accessible.name: qsTr("Launch setup summary and warnings")
    Accessible.description: qsTr("Use arrow keys and Page Up or Page Down to scroll; Tab to reach the controls.")
    background: Rectangle {
        color: "transparent"
        border.color: root.activeFocus ? Theme.accent : "transparent"
        border.width: 2
        Accessible.ignored: true
    }
    Keys.onPressed: event => {
        const view = root.contentItem
        const step = Math.max(24, Theme.body * 2)
        let position = view.contentY
        switch (event.key) {
        case Qt.Key_Up: position -= step; break
        case Qt.Key_Down: position += step; break
        case Qt.Key_PageUp: position -= view.height * 0.85; break
        case Qt.Key_PageDown: position += view.height * 0.85; break
        case Qt.Key_Home: position = view.originY; break
        case Qt.Key_End: position = view.originY + Math.max(0, view.contentHeight - view.height); break
        default: event.accepted = false; return
        }
        view.cancelFlick()
        view.contentY = position
        view.returnToBounds()
        event.accepted = true
    }
    ColumnLayout {
        width: root.availableWidth
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            visible: !root.ready.summary
            text: qsTr("No setup summary available.")
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.ready.summary
            text: qsTr("%1 · Profile: %2").arg(LaunchPresentation.mode(root.identity.mode)).arg(root.identity.profileName)
            textFormat: Text.PlainText
            font.weight: Font.DemiBold
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.ready.summary
            text: root.changed
                ? qsTr("%1 packages · %2 files to copy · %3 bytes")
                    .arg(LaunchPresentation.number(root.summary.packageCount))
                    .arg(LaunchPresentation.number(root.summary.physicalFileCount))
                    .arg(LaunchPresentation.number(root.summary.totalBytes))
                : qsTr("The setup is unchanged. No files will be copied or mounts changed.")
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.ready.summary && root.changed
            text: qsTr("The previous setup remains available to restore.")
            wrapMode: Text.Wrap
        }
        Label {
            objectName: "launchPerformance"
            Layout.fillWidth: true
            visible: !!root.ready.summary && root.changed && text !== ""
            text: LaunchPresentation.performance(root.summary.performance)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !!root.ready.summary && root.installedFonts
            text: qsTr("Installed game fonts are preserved.")
            wrapMode: Text.Wrap
        }
        Repeater {
            model: root.summary.warnings || []
            delegate: Label {
                required property var modelData
                Layout.fillWidth: true
                text: qsTr("Warning: %1").arg(modelData.message)
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
        }
        Button {
            id: details
            visible: !!root.ready.summary
            text: checked ? qsTr("Hide technical details") : qsTr("Show technical details")
            checkable: true
        }
        Label {
            Layout.fillWidth: true
            visible: details.visible && details.checked
            text: qsTr("Game folder: %1").arg(root.summary.gamePath || "")
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        Repeater {
            model: details.checked ? LaunchPresentation.summaryRows(root.summary) : []
            delegate: Label {
                required property string modelData
                Layout.fillWidth: true
                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
        }
    }
}
