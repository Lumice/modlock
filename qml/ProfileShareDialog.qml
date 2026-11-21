pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs as NativeDialogs
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    objectName: "profileShareDialog"
    required property LibraryModel library
    property string profileId: ""
    property bool importing: false
    function variantNames(options) {
        return (options || []).filter(n => !/^payload-[0-9a-f]{64}$/.test(n) && n !== "default" && n !== "Original variant" && !/^pak\d+(?:_dir)?\.vpk$/i.test(n))
            .map(n => n.replace(/_dir\.vpk$/i, "").replace(/\.vpk$/i, "").replace(/[_-]/g, " "))
    }
    ProfileShareController { id: controller; objectName: "profileShareController"; library: root.library; onCreated: root.close() }
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(720, parent ? parent.width - 32 : 720)
    height: Math.min(680, parent ? parent.height - 32 : 680)
    modal: true
    title: importing ? qsTr("Import shared profile") : qsTr("Share profile")
    standardButtons: Dialog.Close
    Overlay.modal: Rectangle { color: Theme.overlay }
    Component.onCompleted: { open(); if (!importing) controller.exportProfile(profileId, false) }
    NativeDialogs.FileDialog {
        id: profileFile
        title: qsTr("Choose a shared profile")
        onAccepted: code.text = controller.readFile(selectedFile)
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: root.importing ? qsTr("Paste a profile code or choose a file. Review missing mods in Downloads, then review again. Creating a profile does not apply it to the game.")
                : qsTr("Share your mods, variants and load order. Local files and device details are excluded.")
            wrapMode: Text.Wrap
            textFormat: Text.PlainText
            color: Theme.secondary
        }
        CheckBox {
            visible: !root.importing
            text: qsTr("Include names of mods that cannot be shared")
            enabled: !controller.busy
            onToggled: controller.exportProfile(root.profileId, checked)
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: 88
            TextArea {
                id: code
                objectName: "profileShareCode"
                readOnly: !root.importing
                text: root.importing ? "" : (controller.result.code || "")
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.WrapAnywhere
                selectByMouse: true
                Accessible.name: root.importing ? qsTr("Shared profile text") : qsTr("Copyable profile code")
                onTextChanged: if (root.importing) controller.invalidate()
            }
        }
        Label {
            visible: !root.importing && !!controller.result.code
            text: controller.result.omitted > 0 ? (controller.result.count === 1 ? qsTr("%1 mod · %2 local-only not included") : qsTr("%1 mods · %2 local-only not included")).arg(controller.result.count || 0).arg(controller.result.omitted) : (controller.result.count === 1 ? qsTr("1 mod") : qsTr("%1 mods").arg(controller.result.count || 0))
            textFormat: Text.PlainText
            Layout.fillWidth: true
            color: Theme.secondary
        }
        RowLayout {
            Layout.fillWidth: true
            Button {
                objectName: "reviewSharedProfileButton"
                text: root.importing ? qsTr("Review profile") : qsTr("Copy code")
                enabled: !controller.busy && (root.importing ? code.text.trim().length > 0 : !!controller.result.code)
                onClicked: root.importing ? controller.review(code.text) : controller.copyCode()
            }
            Button {
                visible: root.importing
                text: qsTr("Choose file…")
                enabled: !controller.busy
                onClicked: profileFile.open()
            }
            BusyIndicator { running: controller.busy; visible: running; Layout.preferredWidth: 28; Layout.preferredHeight: 28 }
        }
        Label {
            visible: !!controller.error
            text: controller.error
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            Layout.fillWidth: true
            color: Theme.warning
        }
        ScrollView {
            id: reviewView
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: availableWidth
            ColumnLayout {
                width: reviewView.availableWidth
                Repeater {
                    model: controller.result.rows || []
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        ModPreview {
                            objectName: "sharedProfileThumbnail"
                            Layout.preferredWidth: 64
                            Layout.preferredHeight: 48
                            source: modelData.thumbnailUrl || ""
                            contextKey: root.library.dataDir + ":profile-share:" + (modelData.title || "")
                            placeholderTitle: modelData.title || ""
                            compact: true
                            decodeSize: Qt.size(128, 96)
                        }
                        Label {
                            objectName: "sharedProfileModTitle"
                            Layout.fillWidth: true
                            text: (modelData.title || "")
                                + (root.importing ? (root.variantNames(modelData.options).length ? "\n" + root.variantNames(modelData.options).join(", ") : "") + "\n"
                                    + (modelData.status === "inLibrary" ? qsTr("In your library") : modelData.status === "unavailable" ? qsTr("Not available") : qsTr("Will be downloaded (review in Downloads)"))
                                    : modelData.omitted ? "\n" + qsTr("Local-only · not included") : "")
                                + (modelData.latest ? "\n" + qsTr("Latest file — the original file could not be matched") : "")
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                        }
                        Button {
                            text: qsTr("Review download…")
                            visible: root.importing && modelData.status !== "inLibrary"
                            enabled: !controller.busy && root.library.remote.canResolve
                            onClicked: controller.download(modelData.submission, modelData.fileId)
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: (controller.result.notShareable || []).length > 0
                    text: qsTr("Unavailable local-only mods (not imported): %1").arg((controller.result.notShareable || []).join(", "))
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.warning
                }
            }
        }
        RowLayout {
            visible: root.importing
            Layout.fillWidth: true
            TextField { id: profileName; Layout.fillWidth: true; placeholderText: qsTr("New profile name"); maximumLength: 120; Accessible.name: placeholderText }
            Button {
                objectName: "createSharedProfileButton"
                text: qsTr("Create profile")
                highlighted: true
                enabled: !controller.busy && !!controller.result.canCreate && profileName.text.trim().length > 0 && root.library.canEditProfiles
                onClicked: controller.create(code.text, profileName.text.trim())
            }
        }
    }
}
