pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    required property LibraryModel library
    property string targetId: ""
    property string targetName: ""
    property string targetRoot: ""
    property string targetContext: ""
    property string targetRevision: ""
    property string actionMode: "create"
    property string admissionError: ""
    property bool actionStarted: false
    readonly property var target: library.profiles.find(p => p.id === targetId) || ({})
    readonly property bool contextCurrent: targetRoot === library.dataDir
        && targetContext === library.launch.settingsContextId && targetRevision === library.revision
        && targetId === library.viewedProfileId && target.id === targetId && target.name === targetName
    readonly property bool canRemove: contextCurrent && library.canEditProfiles
        && !target.protected && library.appliedProfileId !== targetId
    function selectMode(mode) {
        actionMode = mode
        admissionError = ""
        name.text = mode === "rename" ? targetName : mode === "duplicate" ? qsTr("%1 copy").arg(targetName) : ""
        name.forceActiveFocus()
        name.selectAll()
    }
    function useProfile() {
        const game = root.library.gamePath || root.library.detectedGamePath
        if (!root.contextCurrent || !root.library.canPreviewApply || !game) return
        if (root.library.previewApply(game)) root.close()
        else root.admissionError = qsTr("Could not review this profile. Try again.")
    }
    function submit() {
        if (!contextCurrent || !library.canEditProfiles || !name.text.trim()) return
        const value = name.text.trim()
        const accepted = actionMode === "create" ? library.createProfile(value)
                       : actionMode === "duplicate" ? library.copyProfile(targetId, value)
                       : library.renameProfile(targetId, value)
        if (accepted) actionStarted = true
        else admissionError = qsTr("The profile action could not be started. Check the current library state.")
    }
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(500, parent ? parent.width - 32 : 500)
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    title: qsTr("Profiles")
    standardButtons: Dialog.Close
    Component.onCompleted: {
        targetId = library.viewedProfileId
        targetRoot = library.dataDir
        targetContext = library.launch.settingsContextId
        targetRevision = library.revision
        targetName = target.name || targetId
        open()
        name.forceActiveFocus()
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing16
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: profileCard.implicitHeight + Theme.spacing24
            radius: Theme.radiusCard
            color: Theme.surface
            border.color: Theme.rule
            RowLayout {
                id: profileCard
                anchors.fill: parent
                anchors.margins: Theme.spacing12
                spacing: Theme.spacing12
                Rectangle {
                    Layout.preferredWidth: 36
                    Layout.preferredHeight: 36
                    radius: 18
                    color: Theme.brassSoft
                    Label { anchors.centerIn: parent; text: root.targetName.charAt(0).toUpperCase(); color: Theme.accentInk; font.weight: Font.Bold }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    Label {
                        Layout.fillWidth: true
                        text: root.targetName
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        font.weight: Font.DemiBold
                    }
                    Flow {
                        Layout.fillWidth: true
                        spacing: 6
                        StatusPill { text: root.target.modCount === 1 ? qsTr("1 mod") : qsTr("%1 mods").arg(root.target.modCount || 0) }
                        StatusPill { visible: root.target.id === root.library.activeProfileId; text: qsTr("Active"); tone: "accent" }
                        StatusPill { visible: root.library.appliedProfileId === root.targetId; text: qsTr("In game"); tone: "success" }
                        StatusPill { visible: false; text: qsTr("Protected"); tone: "muted" }
                    }
                }
            }
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Play uses %1.").arg((root.library.profiles.find(p => p.id === root.library.activeProfileId) || {}).name || qsTr("the active profile"))
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Button {
            objectName: "useProfileButton"
            text: qsTr("Use this profile…")
            enabled: root.contextCurrent && root.library.canPreviewApply && !!(root.library.gamePath || root.library.detectedGamePath)
            onClicked: root.useProfile()
        }
        Label {
            Layout.fillWidth: true
            visible: !(root.library.gamePath || root.library.detectedGamePath)
            text: qsTr("Choose the Deadlock folder in Settings before using this profile.")
            color: Theme.secondary
            wrapMode: Text.Wrap
        }
        RowLayout {
            Layout.fillWidth: true
            Button {
                objectName: "shareProfileButton"
                text: qsTr("Share profile…")
                enabled: root.contextCurrent && root.library.canEditProfiles
                onClicked: { sharing.importing = false; sharing.active = true }
            }
            Button {
                objectName: "importSharedProfileButton"
                text: qsTr("Import shared profile…")
                enabled: root.contextCurrent && root.library.canEditProfiles
                onClicked: { sharing.importing = true; sharing.active = true }
            }
        }
        TabBar {
            id: mode
            Layout.fillWidth: true
            Accessible.name: qsTr("Profile action")
            currentIndex: root.actionMode === "create" ? 0 : root.actionMode === "duplicate" ? 1 : 2
            TabButton { text: qsTr("New profile"); icon.source: Theme.icon("plus"); onClicked: root.selectMode("create") }
            TabButton { text: qsTr("Duplicate"); icon.source: Theme.icon("layers"); onClicked: root.selectMode("duplicate") }
            TabButton { text: qsTr("Rename"); icon.source: Theme.icon("edit"); onClicked: root.selectMode("rename") }
        }
        Label {
            Layout.fillWidth: true
            text: root.actionMode === "create" ? qsTr("Start an empty profile.")
                : root.actionMode === "duplicate" ? qsTr("Copy %1, including its mod choices.").arg(root.targetName)
                : qsTr("Give %1 a new name.").arg(root.targetName)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            TextField {
                id: name
                objectName: "profileNameField"
                Layout.fillWidth: true
                placeholderText: root.actionMode === "rename" ? qsTr("New name") : qsTr("Profile name")
                Accessible.name: placeholderText
                maximumLength: 120
                onAccepted: root.submit()
            }
            Button {
                objectName: root.actionMode === "create" ? "createProfileButton" : root.actionMode === "duplicate" ? "copyProfileButton" : "renameProfileButton"
                highlighted: true
                text: root.actionMode === "create" ? qsTr("Create") : root.actionMode === "duplicate" ? qsTr("Duplicate") : qsTr("Rename")
                enabled: root.contextCurrent && root.library.canEditProfiles && name.text.trim().length > 0
                onClicked: root.submit()
            }
        }
        Label {
            Layout.fillWidth: true
            visible: !root.contextCurrent || !!root.admissionError
            text: root.admissionError || qsTr("The viewed profile or library changed. Close this dialog and open it again.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.warning
        }
        ScrollView {
            id: resultView
            Layout.fillWidth: true
            Layout.preferredHeight: visible ? Math.min(80, resultText.implicitHeight) : 0
            visible: root.actionStarted && (root.library.busy || root.library.outcomeUnknown || !!root.library.error || !!root.library.status)
            clip: true
            contentWidth: availableWidth
            Label {
                id: resultText
                width: resultView.availableWidth
                text: Theme.friendlyError(root.library.error) || (root.library.busy ? qsTr("Saving profile…")
                    : root.library.outcomeUnknown ? qsTr("The profile result needs checking in Library.") : root.library.status)
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                color: Theme.secondary
            }
        }
        Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Theme.rule }
        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacing12
            Label {
                Layout.fillWidth: true
                text: root.target.protected ? qsTr("The default profile can't be removed.")
                    : root.library.appliedProfileId === root.targetId ? qsTr("The profile that's in the game can't be removed.")
                    : qsTr("Removing deletes this profile's mod choices. Your mods stay in the library.")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.secondary
                font.pointSize: Theme.small
            }
            Button {
                id: removeButton
                objectName: "removeProfileButton"
                text: qsTr("Remove…")
                icon.source: Theme.icon("trash")
                icon.color: enabled ? Theme.danger : Theme.faint
                flat: true
                enabled: root.canRemove
                Accessible.name: qsTr("Remove %1").arg(root.targetName)
                onClicked: removeConfirmation.open()
            }
        }
    }
    Loader {
        id: sharing
        property bool importing: false
        active: false
        sourceComponent: ProfileShareDialog {
            library: root.library
            profileId: root.targetId
            importing: sharing.importing
            onClosed: sharing.active = false
        }
    }
    Dialog {
        id: removeConfirmation
        objectName: "removeProfileConfirmation"
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(420, parent ? parent.width - 32 : 420)
        modal: true
        Overlay.modal: Rectangle { color: Theme.overlay }
        title: qsTr("Remove profile?")
        standardButtons: Dialog.Yes | Dialog.Cancel
        onOpened: standardButton(Dialog.Cancel).forceActiveFocus()
        contentItem: Label {
            text: qsTr("Remove %1 and its saved mod choices? Your mods stay in the library.").arg(root.targetName)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        onAccepted: {
            if (!root.canRemove) { root.admissionError = qsTr("The profile or library changed. Open Profiles again."); return }
            if (root.library.removeProfile(root.targetId)) root.actionStarted = true
            else root.admissionError = qsTr("The profile could not be removed. Check the current library state.")
        }
        onRejected: name.forceActiveFocus()
    }
}
