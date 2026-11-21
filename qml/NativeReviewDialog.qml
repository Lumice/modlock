pragma ComponentBehavior: Bound
import QtQuick
import "HealthText.js" as HealthText
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Deadlock.Native
import "LaunchPresentation.js" as LaunchPresentation

Dialog {
    id: root
    required property LibraryModel library
    required property string workflow
    required property string sourcePath
    required property string gamePath
    readonly property bool importing: workflow.startsWith("import")
    readonly property bool reviewing: workflow.endsWith("-review")
    readonly property bool restoring: workflow === "restore"
    readonly property var review: importing ? library.importReview : library.applyReview
    readonly property bool hasFamilyCounts: review.packageCount !== undefined
    readonly property bool sharded: review.summaryVersion === 2 && review.deploymentLayout === "shards-v1"
    property bool detailsExpanded: false
    readonly property var conflicts: (review.warnings || []).filter(w => w.code === "fileConflict")
    readonly property string fileSummary: !importing ? (library.enabledCount === 1 ? qsTr("%1 mod · %2 to copy") : qsTr("%1 mods · %2 to copy")).arg(library.enabledCount).arg(Theme.bytes(review.totalBytes)) : hasFamilyCounts ? [
        (sharded ? qsTr("%1 selected package choices") : review.packageCount === 1 ? qsTr("%1 package") : qsTr("%1 packages")).arg(review.packageCount),
        qsTr("%1 unique directory VPKs").arg(review.directoryCount),
        (review.companionFileCount === 1 ? qsTr("%1 companion file") : qsTr("%1 companion files")).arg(review.companionFileCount),
        (review.physicalFileCount === 1 ? qsTr("%1 file to copy") : qsTr("%1 files to copy")).arg(review.physicalFileCount)
    ].join(" · ")
        : importing ? "" : qsTr("%1 VPK files").arg(review.fileCount)
    signal previewRequested(string source, string game)
    signal changeFolderRequested()
    signal commitRequested()
    signal cancelled()
    parent: Overlay.overlay
    width: Math.min(parent.width - 40, 640)
    height: Math.min(parent.height - 40, body.implicitHeight + header.implicitHeight + footer.implicitHeight + topPadding + bottomPadding)
    x: (parent.width - width) / 2
    y: (parent.height - height) / 2
    modal: true
    title: restoring ? qsTr("Restore previous setup") : importing ? (reviewing ? qsTr("Review import") : qsTr("Import from Deadlock Mod Manager")) : (reviewing ? qsTr("Apply changes to the game") : qsTr("Choose your Deadlock folder"))
    onRejected: cancelled()
    Component.onCompleted: open()
    FolderDialog {
        id: gameFolder
        title: qsTr("Choose the Steam Deadlock folder")
        onAccepted: game.text = root.library.localPath(selectedFolder)
    }
    contentItem: ScrollView {
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            id: body
            width: parent.width
            spacing: 12
            Label {
                Layout.fillWidth: true
                text: root.restoring ? qsTr("Put the game back to the mods it used before the last Apply. Your profile choices stay saved, so you can apply them again later. Installed fonts are not changed.")
                    : root.importing ? qsTr("Copies your mods from Deadlock Mod Manager into this app. Your original library and files are left untouched.")
                    : root.reviewing ? qsTr("These mods will be copied into the game. The current setup is kept so you can restore it. Close Deadlock first.")
                    : qsTr("Apply copies this profile's mods into the game. The current setup is kept so you can restore it. Close Deadlock first.")
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            Label { text: qsTr("Deadlock Mod Manager library file (state.json)"); font.weight: Font.Medium; visible: root.importing && !root.reviewing }
            TextField {
                id: source
                objectName: "importSourcePath"
                Layout.fillWidth: true
                visible: root.importing && !root.reviewing
                text: root.sourcePath
                Accessible.name: qsTr("Upstream state.json path")
                selectByMouse: true
            }
            Label {
                Layout.fillWidth: true
                visible: root.workflow === "apply" && root.library.error !== ""
                text: Theme.friendlyError(root.library.error)
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label { text: qsTr("Deadlock game folder"); font.weight: Font.Medium; visible: !root.reviewing && !root.restoring }
            RowLayout {
                Layout.fillWidth: true
                visible: !root.reviewing && !root.restoring
                TextField {
                    id: game
                    objectName: "gameFolderPath"
                    Layout.fillWidth: true
                    text: root.gamePath
                    placeholderText: qsTr("Choose the actual Steam installation")
                    Accessible.name: qsTr("Steam Deadlock folder path")
                    selectByMouse: true
                }
                Button {
                    id: installsButton
                    visible: root.library.detectedGamePaths.length > 1
                    text: qsTr("Steam installs")
                    icon.source: Theme.icon("chevron-down")
                    onClicked: installsMenu.popup(installsButton, 0, installsButton.height + 4)
                    Menu {
                        id: installsMenu
                        Repeater {
                            model: root.library.detectedGamePaths
                            MenuItem { required property string modelData; text: modelData; onTriggered: game.text = modelData }
                        }
                    }
                }
                Button { text: qsTr("Choose game folder…"); icon.source: Theme.icon("folder"); onClicked: gameFolder.open() }
            }
            Label {
                Layout.fillWidth: true
                visible: !root.reviewing && !root.restoring
                text: (game.text !== "" && root.library.detectedGamePaths.includes(game.text)
                       ? qsTr("Found in your Steam library. ")
                       : qsTr("Usually …/steamapps/common/Deadlock. "))
                      + qsTr("Checking large mods can take a few minutes.")
                font.pointSize: Theme.small
                wrapMode: Text.Wrap
                color: Theme.secondary
            }
            Frame {
                Layout.fillWidth: true
                visible: root.reviewing
                Label {
                objectName: "reviewSummary"
                width: parent.width
                font.weight: Font.Medium
                visible: root.reviewing
                text: (root.importing ? qsTr("%1 profiles · %2 mods\n%3 available · %4 unavailable").arg(root.review.profileCount || 0).arg(root.review.modCount || 0).arg(root.review.availableModCount || 0).arg(root.review.unavailableModCount || 0)
                    : qsTr("Profile: %1").arg((root.library.profiles.find(p => p.id === root.review.profileId) || {}).name || ""))
                    + (root.fileSummary ? "\n" + root.fileSummary : "")
                    + (root.importing ? "\n" + qsTr("Copy size: %1").arg(Theme.bytes(root.review.totalBytes)) : "")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                lineHeight: 1.2
                }
            }
            Label {
                Layout.fillWidth: true
                visible: root.reviewing && root.sharded && root.review.shardCount >= 8
                text: qsTr("Nearly at the game’s mod limit: %1 of 10 folders used. Disable some mods if you reach the limit.").arg(root.review.shardCount)
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                objectName: "reviewPerformance"
                Layout.fillWidth: true
                readonly property var perf: root.review.performance || null
                visible: root.reviewing && !root.restoring && !!perf && text !== ""
                text: LaunchPresentation.performance(perf)
                wrapMode: Text.Wrap
                textFormat: Text.PlainText
                color: Theme.text
            }
            Label {
                objectName: "reviewRepairs"
                Layout.fillWidth: true
                visible: root.reviewing && (root.review.repairModCount || 0) > 0
                text: (root.review.repairModCount === 1 ? qsTr("Fixes %1 mod for this patch: %2.") : qsTr("Fixes %1 mods for this patch: %2.")).arg(root.review.repairModCount || 0).arg(HealthText.symptoms(root.review.repairSymptoms))
                wrapMode: Text.Wrap
                textFormat: Text.PlainText
            }
            Label {
                objectName: "reviewMountMigration"
                Layout.fillWidth: true
                visible: root.reviewing && !root.importing && ((root.review.replacedMounts || []).length > 0 || !!root.review.upstreamRepairMigration)
                text: root.review.upstreamRepairMigration === "replaceMountPreserveFolder"
                    ? qsTr("Replaces the current mods setup and the old Mod Manager’s repair folder. All existing files are kept.")
                    : qsTr("Replaces the previous mods in the game setup. Their files are kept for Restore.")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Button {
                objectName: "reviewDetailsToggle"
                visible: root.reviewing && !root.importing
                text: root.detailsExpanded ? qsTr("Hide details") : qsTr("Details")
                flat: true
                onClicked: root.detailsExpanded = !root.detailsExpanded
            }
            RowLayout {
                Layout.fillWidth: true
                visible: root.reviewing
                Label {
                    objectName: "reviewPaths"
                    Layout.fillWidth: true
                    text: qsTr("Game: %1").arg(root.review.gamePath || "") + (root.importing ? qsTr("\nSource: %1").arg(root.sourcePath) : "")
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    color: Theme.secondary
                    font.pointSize: Theme.small
                }
                Button {
                    objectName: "changeApplyFolder"
                    visible: !root.importing
                    text: qsTr("Change folder…")
                    enabled: !root.library.busy
                    onClicked: root.changeFolderRequested()
                }
            }
            Label {
                objectName: "reviewReasons"
                Layout.fillWidth: true
                visible: root.reviewing && (root.importing || root.detailsExpanded)
                color: Theme.secondary
                font.pointSize: Theme.small
                text: root.importing ? (root.review.profiles || []).map(p => qsTr("%1: %2 mods").arg(p.name).arg(p.modCount)).join("\n")
                    + "\n" + (root.review.unavailableMods || []).map(m => qsTr("%1: %2").arg(m.title).arg(m.reason)).join("\n")
                    : qsTr("Mounts replaced:\n%1").arg((root.review.replacedMounts || []).join("\n") || qsTr("None"))
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: root.reviewing && (root.review.summariesTruncated || false)
                text: qsTr("The explanation list is shortened; totals include all entries.")
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: root.reviewing && root.conflicts.length > 0
                text: (root.review.warnings || []).some(w => w.code === "additionalConflicts")
                    ? qsTr("%1 overlapping file entries shown; additional conflicts are not shown").arg(root.conflicts.length)
                    : qsTr("%1 overlapping file entries").arg(root.conflicts.length)
                wrapMode: Text.Wrap
            }
            Disclosure {
                id: conflictDisclosure
                Layout.fillWidth: true
                visible: root.reviewing && root.conflicts.length > 0
                label: qsTr("Conflicting files (%1)").arg(root.conflicts.length)
            }
            ColumnLayout {
                Layout.fillWidth: true
                visible: conflictDisclosure.checked && root.reviewing
                Repeater {
                    model: root.conflicts
                    Label {
                        required property var modelData
                        Layout.fillWidth: true
                        text: modelData.message || ""
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                }
            }
            Label {
                Layout.fillWidth: true
                visible: root.reviewing
                objectName: "reviewWarnings"
                text: (root.review.warnings || []).filter(w => w.code !== "fileConflict").map(w => w.message).join("\n")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
        }
    }
    footer: DialogButtonBox {
        Button { objectName: "cancelReviewButton"; text: qsTr("Cancel"); onClicked: root.reject() }
        Button {
            objectName: root.reviewing ? (root.importing ? "importCommitButton" : "applyCommitButton") : root.restoring ? "restoreCommitButton" : root.importing ? "reviewImportButton" : "reviewApplyButton"
            text: root.restoring ? qsTr("Restore") : root.reviewing ? (root.importing ? qsTr("Import") : qsTr("Apply")) : (!root.importing && game.text.trim() !== root.library.gamePath ? qsTr("Use this game folder") : qsTr("Review"))
            highlighted: true
            enabled: !root.library.busy && (root.restoring ? root.library.canRestore : root.reviewing ? !!root.review.token : game.text.trim().length > 0 && (!root.importing || source.text.trim().length > 0))
            onClicked: {
                if (root.restoring || root.reviewing) root.commitRequested()
                else root.previewRequested(source.text.trim(), game.text.trim())
            }
        }
    }
}
