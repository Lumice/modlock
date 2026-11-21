pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    objectName: "modRemovalDialog"
    required property LibraryModel library
    property var modIds: []
    property bool deleting: false
    property string capturedRoot: ""
    property string capturedProfile: ""
    property string capturedRevision: ""
    property bool invalidated: false
    property string capturedName: ""
    readonly property bool contextCurrent: !invalidated && modIds.length > 0
        && library.dataDir === capturedRoot && library.viewedProfileId === capturedProfile
        && library.revision === capturedRevision
    readonly property bool canConfirm: contextCurrent && (deleting ? library.canDeleteMods : library.canBulkOperate)
    function openFor(ids, deleteLibrary) {
        if (!ids.length || !(deleteLibrary ? library.canDeleteMods : library.canBulkOperate)) return
        modIds = ids.slice()
        deleting = deleteLibrary
        capturedRoot = library.dataDir
        capturedProfile = library.viewedProfileId
        capturedRevision = library.revision
        invalidated = false
        capturedName = ids.length === 1 ? String(library.modDetails(ids[0]).title || "") : ""
        open()
    }
    Connections {
        target: root.library
        function onSourceChanged() { if (root.visible) root.invalidated = true }
    }
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(440, parent ? parent.width - 32 : 440)
    modal: true
    title: deleting ? (modIds.length === 1 ? qsTr("Delete mod from the library?") : qsTr("Delete %n mods from the library?", "", modIds.length))
                    : (modIds.length === 1 ? qsTr("Remove mod from this profile?") : qsTr("Remove %n mods from this profile?", "", modIds.length))
    standardButtons: Dialog.Ok | Dialog.Cancel
    Component.onCompleted: {
        standardButton(Dialog.Ok).objectName = "confirmModRemoval"
        standardButton(Dialog.Cancel).objectName = "cancelModRemoval"
        standardButton(Dialog.Ok).text = Qt.binding(() => root.deleting ? qsTr("Delete") : qsTr("Remove"))
        standardButton(Dialog.Ok).enabled = Qt.binding(() => root.canConfirm)
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            visible: root.capturedName !== ""
            text: root.capturedName
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.weight: Font.DemiBold
        }
        Label {
            Layout.fillWidth: true
            text: root.deleting
                ? qsTr("Removes these mods from every profile and deletes unused files the app can confirm it created. Original imports, installed fonts, shared files and files needed for restore are kept.")
                : qsTr("Removes these mods from this profile. Other profiles and files stay.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            text: root.capturedProfile === root.library.activeProfileId
                ? qsTr("Changes to the game follow your Apply setting.")
                : root.deleting ? qsTr("Your active game setup updates on its next Apply or Play.")
                                : qsTr("Your active game setup stays unchanged.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !root.contextCurrent
            text: qsTr("The library or profile changed. Close this dialog and select the mods again.")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.danger
        }
    }
    onAccepted: {
        if (!canConfirm) return
        if (deleting) library.deleteMods(modIds)
        else library.removeModsFromProfile(modIds)
    }
}
