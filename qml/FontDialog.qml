pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
import "FontPresentation.js" as FontPresentation

Dialog {
    id: root
    required property FontController controller
    property var selection: []
    property var detailRows: null
    property string detailTitle: ""
    property bool technicalDetails: false
    readonly property var summary: controller.review.summary || ({})
    readonly property bool reviewed: !!controller.review.token
    function disposition(value) {
        switch (value) {
        case "create": return qsTr("Copy new font file")
        case "borrow": return qsTr("Use identical existing file; leave it in place on removal")
        case "share": return qsTr("Use the installed font file")
        case "replace": return qsTr("Replace the installed font file")
        case "blocked": return qsTr("Blocked by the reasons below")
        case "delete": return qsTr("Remove the font file when it is no longer needed")
        case "releaseBorrowed": return qsTr("Keep the existing font file")
        case "releaseOwner": return qsTr("Keep the shared font file")
        case "writeExact": return qsTr("Write the reviewed file contents and permissions")
        case "preserveBorrowed": return qsTr("Preserve the borrowed file")
        case "preserveFile": return qsTr("Preserve the unchanged file")
        case "restoreExact": return qsTr("Restore exact previous contents and permissions")
        case "restoreAbsence": return qsTr("Restore previous absence")
        default: return ""
        }
    }
    function status(value) {
        return value === "installed" ? qsTr("Installed") : value === "borrowed" ? qsTr("Borrowed existing file") : qsTr("Stashed in library")
    }
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(820, parent ? parent.width - 32 : 820)
    height: Math.min(700, parent ? parent.height - 32 : 700)
    modal: false
    title: controller.kind === "undo" ? qsTr("Undo last font change") : controller.kind === "remove" ? qsTr("Remove installed fonts") : qsTr("Install fonts")
    closePolicy: controller.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    Component.onCompleted: { open(); closeButton.forceActiveFocus() }
    onClosed: controller.close()
    Connections {
        target: root.controller
        function onSelectionReset() { root.selection = [] }
    }
    ColumnLayout {
        anchors.fill: parent
        Label {
            Layout.fillWidth: true
            text: qsTr("Shared game fonts\nGame: %1\nTarget: %1/game/citadel/panorama/fonts\nSource mod: %2").arg(root.controller.gamePath).arg(root.controller.modId || qsTr("Global font history"))
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Fonts are shared by the whole game: they stay installed when you switch profiles, turn mods off or restore a setup. Check the changes below before continuing.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Button { text: qsTr("Details"); checkable: true; checked: root.technicalDetails; onToggled: root.technicalDetails = checked }
        Frame {
            Layout.fillWidth: true
            Layout.fillHeight: true
            padding: 2
            background: Rectangle {
                color: Theme.surface
                border.color: fonts.activeFocus ? Theme.accent : Theme.rule
                border.width: fonts.activeFocus ? 2 : 1
            }
            ListView {
                id: fonts
                anchors.fill: parent
                activeFocusOnTab: true
                keyNavigationEnabled: false
                Accessible.name: qsTr("Font changes")
                Accessible.description: qsTr("Use Up and Down to scroll by pixels, Page Up and Page Down to scroll by pages, and Tab to reach the review controls.")
                Keys.onPressed: event => FontPresentation.scrollKey(fonts, event, Math.max(24, Theme.body * 2))
                clip: true
                reuseItems: true
                spacing: 12
                model: root.reviewed ? root.controller.review.displayRows || [] : root.controller.assets
                ScrollBar.vertical: ScrollBar {}
                header: ColumnLayout {
                    width: fonts.width
                    Label {
                        Layout.fillWidth: true
                        visible: root.reviewed
                        text: qsTr("After this change: %1 font files, %2 bytes. Write: %3 bytes; backup: %4 bytes.\n%5")
                            .arg(Number(root.summary.installedFileCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(root.summary.totalFontBytes || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(root.summary.writeBytes || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(root.summary.backupBytes || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(root.summary.createDirectory ? qsTr("Ensure the game font directory; retain it after rollback or undo.") : qsTr("Game font directories are retained after removal, rollback and undo."))
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.reviewed && root.controller.review.exactPatterns
                        text: qsTr("fonts.conf: %1 → %2").arg((root.summary.patternChanges || {}).configurationBefore ? qsTr("Present") : qsTr("Absent")).arg((root.summary.patternChanges || {}).configurationAfter ? qsTr("Present") : qsTr("Absent"))
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.reviewed
                        text: !root.controller.review.exactPatterns || !root.controller.review.exactOwners ? qsTr("This helper does not provide exact pattern and ownership changes. Font commit is disabled; use the paired newer helper.")
                            : !(root.summary.patternChanges || {}).configurationBefore && !(root.summary.patternChanges || {}).configurationAfter ? qsTr("fonts.conf is absent and remains absent. No app-managed pattern changes.")
                            : !root.controller.review.patternDelta ? qsTr("No app-managed pattern changes. Existing game patterns are preserved.")
                            : qsTr("Exact app-managed pattern changes are listed below. Other game patterns are preserved.")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                }
                delegate: ColumnLayout {
                    id: row
                    required property var modelData
                    width: fonts.width
                    Label {
                        Layout.fillWidth: true
                        visible: row.modelData.message !== undefined
                        text: row.modelData.fontId ? qsTr("Asset %1: %2").arg(row.modelData.fontId).arg(row.modelData.message || "") : row.modelData.message || ""
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.technicalDetails && row.modelData.patternAction !== undefined
                        text: row.modelData.patternAction === "add" ? qsTr("Add fontpattern: %1").arg(row.modelData.patternName || "") : qsTr("Remove fontpattern: %1").arg(row.modelData.patternName || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.technicalDetails && row.modelData.ownerDelta === true
                        text: qsTr("Ownership: %1 → %2 owners; %3 added, %4 released.")
                            .arg(Number(row.modelData.beforeCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(row.modelData.afterCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(row.modelData.addedCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                            .arg(Number(row.modelData.releasedCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.technicalDetails && row.modelData.ownerAction !== undefined
                        text: row.modelData.ownerAction === "add"
                            ? qsTr("Add owner · Target: %1\nMod: %2\nAsset: %3\nFamily: %4").arg(row.modelData.targetName || "").arg(row.modelData.modId || "").arg(row.modelData.assetId || "").arg(row.modelData.displayName || "")
                            : qsTr("Release owner · Target: %1\nMod: %2\nAsset: %3\nFamily: %4").arg(row.modelData.targetName || "").arg(row.modelData.modId || "").arg(row.modelData.assetId || "").arg(row.modelData.displayName || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                    CheckBox {
                        Layout.fillWidth: true
                        visible: !root.reviewed
                        enabled: !root.controller.busy && (checked || root.selection.length < 256)
                        checked: root.selection.includes(row.modelData.fontId)
                        contentItem: Label {
                            text: row.modelData.targetName || ""
                            textFormat: Text.PlainText
                            leftPadding: 30
                            wrapMode: Text.WrapAnywhere
                            verticalAlignment: Text.AlignVCenter
                        }
                        Accessible.name: row.modelData.targetName || ""
                        onToggled: {
                            let ids = root.selection.filter(id => id !== row.modelData.fontId)
                            if (checked) ids.push(row.modelData.fontId)
                            root.selection = ids
                            root.controller.discardReview()
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.reviewed && row.modelData.message === undefined && row.modelData.patternAction === undefined && row.modelData.ownerAction === undefined
                        text: row.modelData.target ? (row.modelData.target.kind === "configuration" ? qsTr("fonts.conf") : row.modelData.target.name) : row.modelData.targetName || ""
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                        font.weight: Font.Medium
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: row.modelData.message === undefined && row.modelData.patternAction === undefined && row.modelData.ownerAction === undefined
                        text: root.reviewed ? root.disposition(row.modelData.disposition) : root.status(row.modelData.status)
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: !!row.modelData.displayName && row.modelData.ownerAction === undefined
                        text: qsTr("Font pattern family: %1").arg(row.modelData.displayName || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.technicalDetails && row.modelData.remainingOwnerCount !== undefined
                        text: qsTr("Remaining owners: %1. Their files and required patterns are preserved.").arg(Number(row.modelData.remainingOwnerCount || 0).toLocaleString(Qt.locale(), 'f', 0))
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.technicalDetails && !!row.modelData.sha256
                        text: qsTr("New SHA-256: %1\nPrevious SHA-256: %2").arg(row.modelData.sha256 || "").arg(row.modelData.beforeSha256 || qsTr("Absent"))
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                        color: Theme.secondary
                    }
                    Button {
                        visible: root.technicalDetails && (row.modelData.origins || []).length + (row.modelData.releasedOwners || []).length > 0
                        text: (row.modelData.releasedOwners || []).length > 0 ? qsTr("Owners affected…") : qsTr("Provenance…")
                        onClicked: {
                            root.detailTitle = row.modelData.targetName || ""
                            root.detailRows = row.modelData.releasedOwners || row.modelData.origins || []
                        }
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: !root.reviewed && root.controller.kind !== "undo"
            text: qsTr("%1 selected. Select up to 256 fonts per change.").arg(Number(root.selection.length).toLocaleString(Qt.locale(), 'f', 0))
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: root.controller.busy || !!root.controller.error
            text: root.controller.error || qsTr("Checking the selected fonts and existing game files…")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
    }
    footer: GridLayout {
        columns: root.width < 650 ? 1 : 3
        Button {
            id: closeButton
            Layout.fillWidth: true
            text: qsTr("Close review")
            enabled: !root.controller.busy
            Keys.onPressed: event => {
                if (event.key === Qt.Key_Backtab) {
                    fonts.forceActiveFocus()
                    event.accepted = true
                } else event.accepted = false
            }
            onClicked: root.close()
        }
        Button {
            Layout.fillWidth: true
            visible: root.controller.kind !== "undo"
            text: root.reviewed ? qsTr("Change selection") : qsTr("Review selected fonts")
            enabled: !root.controller.busy && (root.reviewed || root.selection.length > 0)
            onClicked: { if (root.reviewed) root.controller.discardReview(); else root.controller.preview(root.controller.modId, root.selection, root.controller.kind) }
        }
        Button {
            Layout.fillWidth: true
            highlighted: true
            text: root.controller.kind === "undo" ? qsTr("Undo last font change") : root.controller.kind === "remove" ? qsTr("Remove reviewed fonts") : qsTr("Install reviewed fonts")
            enabled: root.controller.canCommit
            onClicked: root.controller.commit()
        }
    }
    Loader {
        active: root.detailRows !== null && root.visible
        sourceComponent: Dialog {
            id: disclosure
            property var closeAction: null
            parent: Overlay.overlay
            anchors.centerIn: parent
            width: Math.min(650, parent ? parent.width - 48 : 650)
            height: Math.min(450, parent ? parent.height - 48 : 450)
            title: root.detailTitle
            header: Label {
                text: root.detailTitle
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                padding: 16
                font.weight: Font.Medium
            }
            modal: true
            standardButtons: Dialog.Close
            Component.onCompleted: {
                open()
                closeAction = standardButton(Dialog.Close)
                if (closeAction) {
                    closeAction.KeyNavigation.tab = details
                    closeAction.KeyNavigation.backtab = details
                    closeAction.forceActiveFocus()
                }
            }
            onClosed: { root.detailRows = null; closeButton.forceActiveFocus() }
            Frame {
                anchors.fill: parent
                padding: 2
                background: Rectangle {
                    color: Theme.surface
                    border.color: details.activeFocus ? Theme.accent : Theme.rule
                    border.width: details.activeFocus ? 2 : 1
                }
                ListView {
                    id: details
                    anchors.fill: parent
                    activeFocusOnTab: true
                    keyNavigationEnabled: false
                    Accessible.name: root.detailTitle
                    Accessible.description: qsTr("Use Up and Down to scroll by pixels, Page Up and Page Down to scroll by pages, and Tab to reach Close.")
                    KeyNavigation.tab: disclosure.closeAction
                    KeyNavigation.backtab: disclosure.closeAction
                    Keys.onPressed: event => FontPresentation.scrollKey(details, event, Math.max(24, Theme.body * 2))
                    clip: true
                    reuseItems: true
                    spacing: 12
                    model: root.detailRows || []
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Label {
                        required property var modelData
                        width: details.width
                        text: FontPresentation.source(modelData)
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                    }
                }
            }
        }
    }
}
