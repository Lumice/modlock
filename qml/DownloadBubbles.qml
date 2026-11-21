pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

Column {
    id: root
    required property LibraryModel library
    signal libraryRequested(string modId, string profileId)
    objectName: "downloadBubbles"
    width: Math.min(320, parent.width - 32)
    spacing: Theme.spacing8
    property var dismissed: ({})
    readonly property var notices: {
        const jobs = library.remote.jobs.filter(job => job.notice && job.state !== "installed")
        const error = library.remote.noticeError
        return !error || jobs.some(job => job.installError === error || (job.error || {}).message === error) ? jobs
            : [{name: qsTr("Download needs attention"), installError: error, startRejected: true}, ...jobs]
    }
    function noticeKey(job) { return (job.jobId || "error") + ":" + (job.jobId ? job.failureSequence || 0 : job.installError || "") }
    function current(job) { return !dismissed[noticeKey(job)] }
    function dismiss(job) { const copy = Object.assign({}, dismissed); copy[noticeKey(job)] = true; dismissed = copy }
    function clearDismissed(prefix) {
        const keys = Object.keys(dismissed).filter(key => key.startsWith(prefix))
        if (keys.length === 0) return
        const copy = Object.assign({}, dismissed)
        for (const key of keys) delete copy[key]
        dismissed = copy
    }
    Connections {
        target: root.library.remote
        function onRetryStarted(jobId) { root.clearDismissed(jobId + ":") }
        function onChanged() {
            if (!root.library.remote.noticeError) root.clearDismissed("error:")
            for (const job of root.library.remote.jobs)
                if (job.state !== "failed" && !job.installError) root.clearDismissed(job.jobId + ":")
        }
    }
    readonly property int activeCount: notices.filter(job => current(job)).length
    visible: activeCount > 0
    Label { visible: root.activeCount > 3; text: qsTr("%1 downloads").arg(root.activeCount); color: Theme.secondary }
    Repeater {
        model: root.notices
        Rectangle {
            id: notice
            required property var modelData
            required property int index
            readonly property var progress: modelData.progress || ({})
            readonly property real total: Number(progress.total || 0)
            readonly property real fraction: total > 0 ? Math.max(0, Math.min(1, Number(progress.bytesWritten || 0) / total)) : 0
            readonly property bool paused: modelData.state === "paused"
            readonly property int stub: 44
            readonly property string failure: Theme.downloadError(modelData.installError || (modelData.error && (modelData.error.kind || modelData.error.message) ? modelData.error : ""))
            width: root.width
            visible: root.current(modelData) && root.notices.slice(0, index).filter(job => root.current(job)).length < 3
            implicitHeight: body.implicitHeight + 2 * Theme.spacing12
            radius: Theme.radiusCard
            color: Theme.surfaceHigh
            border.color: Theme.ruleStrong
            clip: true
            readonly property bool byteComplete: notice.failure === "" && ["downloaded", "preparing", "ready", "reviewing", "committing", "installed"].includes(modelData.state)
            readonly property bool waitingChoice: !!modelData.choosing || modelData.state === "needsSelection"
            readonly property bool working: notice.failure === "" && !notice.paused
            readonly property bool measured: notice.failure === "" && !notice.byteComplete && notice.total > 0
            readonly property bool indeterminate: notice.working && !notice.byteComplete && !notice.measured
            readonly property bool busyWorking: notice.working && !notice.measured && !modelData.canContinue && !notice.waitingChoice
            readonly property real displayFraction: notice.failure === "" && notice.byteComplete ? 1 : (notice.measured ? notice.fraction : 0)
            ProgressBar {
                objectName: "downloadProgress"
                anchors.fill: parent
                visible: notice.working
                indeterminate: notice.indeterminate
                value: notice.displayFraction
                Accessible.name: qsTr("Download progress for %1").arg(notice.modelData.name || "")
                background: Item {}
                contentItem: Item {}
            }
            // Compact ticket: a placeholder stub (jobs carry no art), a perforated
            // edge, and progress filling the row with a bottom line.
            Rectangle {
                x: 1; y: 1
                width: notice.stub - 1
                height: parent.height - 2
                topLeftRadius: Theme.radiusCard - 1
                bottomLeftRadius: Theme.radiusCard - 1
                color: Theme.surface
                Accessible.ignored: true
                ColorImage {
                    id: bubbleRays
                    anchors.horizontalCenter: parent.horizontalCenter
                    y: Theme.spacing8
                    width: 36; height: 36
                    source: Theme.icon("sunburst-rays")
                    sourceSize: Qt.size(36, 36)
                    color: notice.failure !== "" || notice.paused ? Qt.alpha(Theme.faint, 0.45) : Qt.alpha(Theme.brass, 0.55)
                }
                Rectangle {
                    anchors.centerIn: bubbleRays
                    width: 12; height: 12; radius: 6
                    color: "transparent"
                    border.width: 1.5
                    border.color: notice.failure !== "" || notice.paused ? Theme.faint : Theme.brass
                }
            }
            Column {
                x: notice.stub - 1
                y: 6
                spacing: 3
                Accessible.ignored: true
                Repeater {
                    model: Math.max(0, Math.floor((notice.height - 12 + 3) / 6))
                    Rectangle { width: 1; height: 3; color: Theme.ruleStrong }
                }
            }
            Rectangle {
                objectName: "downloadBubbleFill"
                visible: notice.failure === "" && (notice.measured || notice.byteComplete) && notice.displayFraction > 0
                x: notice.stub
                y: 1
                width: (notice.width - notice.stub - 1) * notice.displayFraction
                height: parent.height - 2
                topRightRadius: notice.displayFraction >= 1 ? Theme.radiusCard - 1 : 0
                bottomRightRadius: topRightRadius
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: notice.paused ? Qt.alpha(Theme.secondary, 0.05) : Qt.alpha(Theme.accent, 0.08) }
                    GradientStop { position: 1; color: notice.paused ? Qt.alpha(Theme.secondary, 0.09) : Qt.alpha(Theme.accent, 0.22) }
                }
                Accessible.ignored: true
            }
            Item {
                id: bubbleLine
                x: notice.stub
                y: parent.height - 4
                width: notice.width - notice.stub - 1 - Theme.radiusCard / 2
                height: 3
                clip: true
                visible: notice.failure === "" && (notice.measured || notice.byteComplete)
                Accessible.ignored: true
                Rectangle {
                    width: Math.min(parent.width, (notice.width - notice.stub - 1) * notice.displayFraction)
                    height: parent.height
                    color: notice.paused ? Theme.faint : Theme.accentInk
                }
            }
            ColumnLayout {
                id: body
                anchors { left: parent.left; right: parent.right; top: parent.top; margins: Theme.spacing12; leftMargin: notice.stub + Theme.spacing12 }
                spacing: Theme.spacing8
                RowLayout {
                    Layout.fillWidth: true
                    Label { Layout.fillWidth: true; text: notice.modelData.name; textFormat: Text.PlainText; elide: Text.ElideRight; font.weight: Font.DemiBold }
                    BusyIndicator {
                        objectName: "downloadBusyIndicator"
                        visible: notice.busyWorking
                        running: visible
                        padding: 0
                        implicitWidth: 16
                        implicitHeight: 16
                        Layout.preferredWidth: 16
                        Layout.preferredHeight: 16
                        Layout.alignment: Qt.AlignVCenter
                    }
                    Label {
                        objectName: "downloadBubblePercent"
                        visible: notice.failure === "" && notice.measured
                        text: qsTr("%1%").arg(Math.floor(100 * notice.fraction))
                        textFormat: Text.PlainText
                        font.family: Theme.displayFamily
                        font.pointSize: Theme.title
                        font.weight: Font.DemiBold
                        font.features: { "tnum": 1 }
                        color: notice.paused ? Theme.faint : Theme.accentInk
                    }
                    ToolButton {
                        objectName: "dismissDownloadBubble"
                        visible: notice.failure !== ""
                        text: qsTr("Dismiss download notice")
                        display: AbstractButton.IconOnly
                        icon.source: Theme.icon("x")
                        implicitWidth: 28; implicitHeight: 28
                        padding: 5
                        Accessible.name: qsTr("Dismiss download notice")
                        ToolTip.visible: hovered || visualFocus
                        ToolTip.text: qsTr("Dismiss")
                        onClicked: root.dismiss(notice.modelData)
                    }
                }
                Label {
                    objectName: "downloadBubbleMessage"
                    Layout.fillWidth: true
                    text: notice.failure !== "" ? notice.failure
                        : notice.modelData.canContinue ? qsTr("Ready to add to library")
                        : notice.waitingChoice ? qsTr("Choose a variant to continue")
                        : notice.paused ? (notice.measured ? qsTr("Paused · %1 / %2").arg(Theme.bytes(notice.progress.bytesWritten || 0)).arg(Theme.bytes(notice.total)) : qsTr("Paused"))
                        : notice.modelData.state === "cancelRequested" ? qsTr("Cancelling…")
                        : (!!notice.modelData.cleanupPending && ["installed", "failed", "cancelled", "abandoned"].includes(notice.modelData.state)) ? qsTr("Finishing up…")
                        : notice.byteComplete ? qsTr("Adding to library…")
                        : notice.measured ? qsTr("%1 / %2").arg(Theme.bytes(notice.progress.bytesWritten || 0)).arg(Theme.bytes(notice.total))
                        : qsTr("Downloading…")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: notice.failure !== "" ? Theme.danger : Theme.secondary
                }
                Button { objectName: "retryDownloadBubble"; visible: notice.failure !== ""; text: notice.modelData.startRejected ? qsTr("Review files") : qsTr("Retry"); onClicked: notice.modelData.startRejected ? root.library.remote.showDownloads() : root.library.remote.retryInstall(notice.modelData.jobId) }
                Button { visible: !!notice.modelData.choosing; text: qsTr("Choose variant"); onClicked: { root.library.remote.selectJob(notice.modelData.jobId); root.library.remote.showDownloads() } }
                Button { flat: true; text: qsTr("View downloads"); onClicked: root.library.remote.showDownloads() }
            }
        }
    }
}
