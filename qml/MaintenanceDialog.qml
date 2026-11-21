pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
Dialog {
    id: root
    objectName: "maintenanceDialog"
    required property LibraryModel library
    signal applyRequested()
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(800, parent ? parent.width - 32 : 800)
    height: Math.min(700, parent ? parent.height - 32 : 700)
    modal: true
    title: qsTr("Check game mod folders")
    Overlay.modal: Rectangle { color: Theme.overlay }
    function space(bytes) {
        const value = Number(bytes || 0)
        if (value < 1024) return qsTr("%1 B").arg(value)
        if (value < 1048576) return qsTr("%1 KB").arg((value / 1024).toFixed(1))
        return qsTr("%1 MB").arg((value / 1048576).toFixed(value < 10485760 ? 1 : 0))
    }
    readonly property var items: controller.result.items || []
    function itemName(item) {
        if (item.kind === "storageCopy") return item.reason
        let name
        if (item.kind === "payload") name = item.title ? qsTr("Unused download of %1").arg(item.title) : qsTr("Unused download (no longer in any profile)")
        else if (item.kind === "retired") name = item.date ? qsTr("Old mod set from %1").arg(Qt.formatDateTime(new Date(Number(item.date) * 1000), "d MMM yyyy")) : qsTr("Old mod set")
        else name = item.path ? item.path.split("/").pop() : qsTr("Mod folder")
        return Number(item.bytes) > 0 ? qsTr("%1 · %2").arg(name).arg(space(item.bytes)) : name
    }
    property bool keptExpanded: false
    property bool detailsExpanded: false
    MaintenanceController { id: controller; objectName: "maintenanceController"; library: root.library }
    Component.onCompleted: { open(); controller.scan() }
    contentItem: ColumnLayout {
        spacing: Theme.spacing12
        Label {
            Layout.fillWidth: true
            text: qsTr("Remove unused downloads and old mod sets to free disk space. Your current setup and Restore files are kept. Cleanup is permanent; removed files cannot be restored.")
            wrapMode: Text.Wrap
            color: Theme.secondary
        }
        Label {
            Layout.fillWidth: true
            visible: !!controller.result.storage
            text: qsTr("Mod storage: %1\n%2 used · %3 free").arg(controller.result.storage?.current?.path || "").arg(root.space(controller.result.storage?.current?.bytes)).arg(root.space(controller.result.storage?.current?.freeBytes))
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
            color: Theme.secondary
        }
        Label {
            Layout.fillWidth: true
            visible: !!controller.result.storage
            text: qsTr("Library data: %1\n%2 used · %3 free").arg(controller.result.storage?.database?.path || "").arg(root.space(controller.result.storage?.database?.bytes)).arg(root.space(controller.result.storage?.database?.freeBytes))
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
            color: Theme.secondary
        }
        BusyIndicator { running: controller.busy; visible: controller.busy; Layout.alignment: Qt.AlignHCenter }
        Label {
            objectName: "maintenanceSummary"
            Layout.fillWidth: true
            visible: !!controller.result.token
            text: controller.result.resume ? qsTr("Cleanup was interrupted. Resume to finish the recorded removal.")
                : controller.result.count > 0 ? qsTr("Leftovers: %1 · %2 to free").arg(root.items.filter(item => item.removable).length).arg(root.space(controller.result.bytes))
                : qsTr("No leftovers to remove.")
            font.pointSize: Theme.title
            wrapMode: Text.Wrap
        }
        Label { Layout.fillWidth: true; visible: !!controller.result.broken; text: qsTr("Applied files are missing or changed. Apply again to rebuild your setup."); color: Theme.warning; wrapMode: Text.Wrap }
        Button { text: qsTr("Apply again…"); visible: !!controller.result.broken; enabled: !controller.busy && root.library.canPreviewApply; onClicked: { root.close(); root.applyRequested() } }
        Label {
            Layout.fillWidth: true
            visible: !!controller.result.warning || !!controller.result.pendingRecovery
            text: controller.result.pendingRecovery ? qsTr("An unfinished operation needs recovery. Use the recovery action before cleaning up.") : (controller.result.warning || "")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.warning
        }
        Button { text: qsTr("Recover unfinished operation…"); visible: !!controller.result.pendingRecovery; enabled: !controller.busy && root.library.canRecover; onClicked: root.library.recover() }
        Label {
            objectName: "maintenanceFreed"
            Layout.fillWidth: true
            visible: controller.freedBytes !== undefined && controller.freedBytes !== null
            text: qsTr("Cleanup complete · %1 freed").arg(root.space(controller.freedBytes))
            color: Theme.success
            wrapMode: Text.Wrap
        }
        ScrollView {
            id: maintenanceScroll
            objectName: "maintenanceScroll"
            contentWidth: availableWidth
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            ColumnLayout {
                width: maintenanceScroll.availableWidth
                spacing: Theme.spacing8
                Label {
                visible: Number(controller.result.unprovenBytes || 0) > 0
                Layout.fillWidth: true
                text: qsTr("Files the app can't confirm it created — kept (%1)").arg(root.space(controller.result.unprovenBytes))
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label {
                visible: Number(controller.result.unprovenBytes || 0) > 0
                Layout.fillWidth: true
                text: qsTr("Downloads from earlier versions can't be cleaned up automatically.")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
            Label { text: qsTr("Can be removed"); font.bold: true }
                Repeater {
                    model: root.items.filter(item => item.removable)
                    Label {
                        required property var modelData
                        Layout.fillWidth: true
                        text: root.itemName(modelData)
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                }
                Label {
                    visible: root.items.filter(item => item.removable).length === 0
                    text: qsTr("Nothing to clean up.")
                    color: Theme.secondary
                }
                Button {
                    objectName: "maintenanceKept"
                    Layout.topMargin: Theme.spacing8
                    text: qsTr("Kept — not created by this app or still in use (%1)").arg(root.items.filter(item => !item.removable && !(item.kind === "unproven" && item.id)).length)
                    checkable: true
                    checked: root.keptExpanded
                    onToggled: root.keptExpanded = checked
                }
                Label {
                    visible: root.keptExpanded
                    Layout.fillWidth: true
                    text: qsTr("These files and folders are kept because the app cannot safely remove them.")
                    color: Theme.secondary
                    wrapMode: Text.Wrap
                }
                Repeater {
                    model: root.keptExpanded ? root.items.filter(item => !item.removable && !(item.kind === "unproven" && item.id)) : []
                    Label {
                        required property var modelData
                        Layout.fillWidth: true
                        text: root.itemName(modelData)
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                }
                Button {
                    objectName: "maintenanceDetails"
                    text: qsTr("Details")
                    checkable: true
                    checked: root.detailsExpanded
                    onToggled: root.detailsExpanded = checked
                }
                Repeater {
                    model: root.detailsExpanded ? (controller.result.rows || []) : []
                    Label {
                        required property var modelData
                        Layout.fillWidth: true
                        text: modelData.path
                        textFormat: Text.PlainText
                        wrapMode: Text.WrapAnywhere
                        color: Theme.secondary
                    }
                }
            }
        }
        Label { Layout.fillWidth: true; visible: controller.error !== ""; text: controller.error; textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Theme.danger }
    }
    footer: DialogButtonBox {
        spacing: Theme.spacing8
        Button { text: qsTr("Check again"); enabled: !controller.busy; onClicked: controller.scan() }
        Button { text: qsTr("Close"); onClicked: root.close() }
        Button { objectName: "maintenanceClean"; highlighted: true; text: controller.result.resume ? qsTr("Resume cleanup") : qsTr("Clean up · %1").arg(root.space(controller.result.bytes)); enabled: !controller.busy && (!!controller.result.canClean || !!controller.result.resume); onClicked: controller.clean() }
    }
}
