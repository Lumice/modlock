pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.impl
import QtQuick.Layouts
import Deadlock.Native
import DeadlockStyle

ColumnLayout {
    id: root
    required property LibraryModel library
    readonly property RemoteController controller: library.remote
    readonly property bool popupOpen: false
    signal closeRequested()
    signal browseRequested()
    function showCleanupPending(job) {
        return !!job.cleanupPending && ["installed", "failed", "cancelled", "abandoned"].includes(job.state)
    }
    function status(job) {
        if (job.canContinue) return qsTr("Ready to add to library")
        if (job.installError || job.state === "failed") return Theme.downloadError(job.installError || job.error || qsTr("Download interrupted. Try again."))
        if (job.state === "paused") return qsTr("Paused")
        if (job.state === "cancelRequested") return qsTr("Cancelling…")
        if (showCleanupPending(job)) return qsTr("Finishing up…")
        if (job.choosing || job.state === "needsSelection") return qsTr("Choose a variant to continue")
        if (["downloaded", "preparing", "ready", "reviewing", "committing", "installed"].includes(job.state)) return qsTr("Adding to library…")
        const progress = job.progress || {}
        return Number(progress.total) > 0 ? qsTr("Downloading %1%").arg(Math.floor(100 * Number(progress.bytesWritten || 0) / Number(progress.total))) : qsTr("Downloading…")
    }
    readonly property bool empty: root.controller.jobs.length === 0
    // Empty state: keep the message and its action together in the middle.
    Item { Layout.fillHeight: true; visible: root.empty }
    EmptyState {
        objectName: "downloadsEmpty"
        Layout.fillWidth: true
        visible: root.empty
        headline: qsTr("Nothing downloading")
        text: qsTr("Mods you download from Browse show up here while they install.")
    }
    Button {
        objectName: "downloadsBrowseButton"
        Layout.alignment: root.empty ? Qt.AlignHCenter : Qt.AlignLeft
        text: qsTr("Browse mods")
        onClicked: root.browseRequested()
    }
    Label {
        objectName: "downloadsError"
        Layout.fillWidth: true
        visible: root.controller.error !== ""
        text: Theme.downloadError(root.controller.error)
        textFormat: Text.PlainText
        horizontalAlignment: root.empty ? Text.AlignHCenter : Text.AlignLeft
        wrapMode: Text.Wrap
        color: Theme.danger
    }
    ScrollView {
        id: scroll
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: root.controller.jobs.length > 0
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            width: scroll.availableWidth
            spacing: Theme.spacing12
            Repeater {
                model: root.controller.jobs
                // A ticket: placeholder stub, perforated edge, progress filling the
                // whole row with a bottom line, and a large percentage when known.
                Rectangle {
                    id: download
                    required property var modelData
                    readonly property bool failed: !modelData.canContinue && (!!modelData.installError || modelData.state === "failed")
                    readonly property bool paused: modelData.state === "paused"
                    readonly property var progress: modelData.progress || ({})
                    readonly property real total: Number(progress.total || 0)
                    readonly property bool byteComplete: !failed && ["downloaded", "preparing", "ready", "reviewing", "committing", "installed"].includes(modelData.state)
                    readonly property bool waitingChoice: !!modelData.choosing || modelData.state === "needsSelection"
                    readonly property bool working: !failed && !paused
                    readonly property bool measured: !failed && !byteComplete && total > 0
                    readonly property bool indeterminate: working && !byteComplete && !measured
                    readonly property bool busyWorking: working && !measured && !modelData.canContinue && !waitingChoice
                    readonly property int percent: total > 0 ? Math.min(100, Math.floor(100 * Number(progress.bytesWritten || 0) / total)) : 0
                    readonly property real fraction: byteComplete ? 1 : (measured ? Math.max(0, Math.min(1, Number(progress.bytesWritten || 0) / total)) : 0)
                    readonly property int stub: width >= 560 ? 112 : 60
                    readonly property real track: width - stub - 1
                    Layout.fillWidth: true
                    implicitHeight: Math.max(78, ticketBody.implicitHeight + 2 * Theme.spacing12)
                    radius: Theme.radiusCard
                    color: Theme.surface
                    border.color: Theme.rule
                    clip: true
                    // Stub: jobs carry no art, so the sunburst placeholder stands in.
                    Rectangle {
                        objectName: "downloadTicketStub"
                        x: 1; y: 1
                        width: download.stub - 1
                        height: parent.height - 2
                        topLeftRadius: Theme.radiusCard - 1
                        bottomLeftRadius: Theme.radiusCard - 1
                        color: Theme.surfaceHigh
                        Accessible.ignored: true
                        clip: true
                        ColorImage {
                            anchors.centerIn: parent
                            width: Math.min(parent.width, parent.height) * 1.4
                            height: width
                            source: Theme.icon("sunburst-rays")
                            sourceSize: Qt.size(width, height)
                            color: download.paused || download.failed ? Qt.alpha(Theme.faint, 0.35) : Qt.alpha(Theme.brass, 0.4)
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 20; height: 20; radius: 10
                            color: Theme.surfaceHigh
                            border.width: 2
                            border.color: download.paused || download.failed ? Theme.faint : Theme.brass
                        }
                    }
                    // Keep Qt's progress role/range/value semantics; the ticket
                    // decorations below are its visual representation.
                    ProgressBar {
                        objectName: "downloadProgress"
                        anchors.fill: parent
                        visible: download.working
                        indeterminate: download.indeterminate
                        value: download.fraction
                        Accessible.name: qsTr("Download progress for %1").arg(download.modelData.name || "")
                        background: Item {}
                        contentItem: Item {}
                    }
                    // Progress fill across the row, with its bottom line.
                    Rectangle {
                        objectName: "downloadTicketFill"
                        visible: (download.measured || download.byteComplete) && download.fraction > 0
                        x: download.stub
                        y: 1
                        width: download.track * download.fraction
                        height: parent.height - 2
                        topRightRadius: download.fraction >= 1 ? Theme.radiusCard - 1 : 0
                        bottomRightRadius: topRightRadius
                        gradient: Gradient {
                            orientation: Gradient.Horizontal
                            GradientStop { position: 0; color: download.paused ? Qt.alpha(Theme.secondary, 0.05) : Qt.alpha(Theme.accent, 0.08) }
                            GradientStop { position: 1; color: download.paused ? Qt.alpha(Theme.secondary, 0.09) : Qt.alpha(Theme.accent, 0.22) }
                        }
                        Accessible.ignored: true
                    }
                    Item {
                        id: bottomLine
                        x: download.stub
                        y: parent.height - 4
                        width: download.track - Theme.radiusCard / 2
                        height: 3
                        clip: true
                        Accessible.ignored: true
                        Rectangle {
                            objectName: "downloadTicketLine"
                            visible: download.measured || download.byteComplete
                            width: Math.min(parent.width, download.track * download.fraction)
                            height: parent.height
                            color: download.paused ? Theme.faint : Theme.accentInk
                        }
                        Rectangle {
                            anchors.fill: parent
                            visible: download.failed
                            color: Qt.alpha(Theme.danger, 0.55)
                        }
                    }
                    // Perforated edge between the stub and the ticket.
                    Column {
                        x: download.stub - 1
                        y: 12
                        spacing: 4
                        Accessible.ignored: true
                        Repeater {
                            model: Math.max(0, Math.floor((download.height - 24 + 4) / 8))
                            Rectangle { width: 2; height: 4; radius: 1; color: Theme.ruleStrong }
                        }
                    }
                    Repeater {
                        model: [-8, download.height - 8]
                        Rectangle {
                            required property real modelData
                            x: download.stub - 8
                            y: modelData
                            width: 16; height: 16; radius: 8
                            color: Theme.window
                            border.color: Theme.rule
                            Accessible.ignored: true
                        }
                    }
                    ColumnLayout {
                        id: ticketBody
                        anchors { left: parent.left; right: parent.right; top: parent.top; leftMargin: download.stub + Theme.spacing16; rightMargin: Theme.spacing12; topMargin: Theme.spacing12 }
                        spacing: Theme.spacing8
                        RowLayout {
                            Layout.fillWidth: true
                            Layout.minimumHeight: 78 - 2 * Theme.spacing12
                            spacing: Theme.spacing12
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                Label { Layout.fillWidth: true; text: download.modelData.name; textFormat: Text.PlainText; wrapMode: Text.Wrap; font.weight: Font.DemiBold }
                                Label {
                                    objectName: "downloadStatus"
                                    readonly property string statusText: root.status(download.modelData)
                                    readonly property string amount: download.measured ? qsTr("%1 of %2").arg(Theme.bytes(download.progress.bytesWritten || 0)).arg(Theme.bytes(download.total)) : ""
                                    Layout.fillWidth: true
                                    // The percentage is shown large beside it, so the line carries the amount instead.
                                    text: amount === "" ? statusText : statusText === qsTr("Downloading %1%").arg(download.percent) ? qsTr("Downloading · %1").arg(amount) : statusText + " · " + amount
                                    textFormat: Text.PlainText
                                    wrapMode: Text.Wrap
                                    font.features: { "tnum": 1 }
                                    color: download.failed ? Theme.danger : Theme.secondary
                                }
                            }
                            BusyIndicator {
                                objectName: "downloadBusyIndicator"
                                visible: download.busyWorking
                                running: visible
                                padding: 0
                                implicitWidth: 18
                                implicitHeight: 18
                                Layout.preferredWidth: 18
                                Layout.preferredHeight: 18
                                Layout.alignment: Qt.AlignVCenter
                            }
                            Label {
                                objectName: "downloadPercent"
                                visible: download.measured
                                text: qsTr("%1%").arg(download.percent)
                                textFormat: Text.PlainText
                                font.family: Theme.displayFamily
                                font.pointSize: Theme.heading
                                font.weight: Font.DemiBold
                                font.features: { "tnum": 1 }
                                color: download.paused ? Theme.faint : Theme.accentInk
                                Accessible.name: qsTr("%1 percent downloaded").arg(download.percent)
                            }
                            Row {
                                spacing: Theme.spacing8
                                Repeater {
                                    model: download.modelData.canContinue ? [{action: "install", label: qsTr("Add to library")}, {action: "cancel", label: qsTr("Cancel")}]
                                        : download.failed ? [{action: "retry", label: qsTr("Retry")}, {action: "dismiss", label: qsTr("Dismiss")}]
                                        : download.modelData.state === "paused" ? [{action: "resume", label: qsTr("Resume")}, {action: "cancel", label: qsTr("Cancel")}]
                                        : ["resolving", "transferring", "discoveringFamily", "sealing"].includes(download.modelData.state) ? [{action: "pause", label: qsTr("Pause")}, {action: "cancel", label: qsTr("Cancel")}]
                                        : ["downloaded", "preparing", "ready", "reviewing", "needsSelection"].includes(download.modelData.state) ? [{action: "cancel", label: qsTr("Cancel")}] : []
                                    Button {
                                        id: action
                                        required property var modelData
                                        readonly property bool iconOnly: modelData.action !== "install"
                                        text: modelData.label
                                        objectName: "downloadAction-" + modelData.action
                                        icon.source: Theme.icon(({install: "plus", pause: "pause", resume: "play", retry: "refresh", cancel: "x", dismiss: "x"})[modelData.action])
                                        display: iconOnly ? AbstractButton.IconOnly : AbstractButton.TextBesideIcon
                                        highlighted: !iconOnly
                                        leftPadding: iconOnly ? 9 : Tokens.controlPadding
                                        rightPadding: iconOnly ? 9 : Tokens.controlPadding
                                        implicitWidth: iconOnly ? Tokens.controlHeight : Math.max(implicitBackgroundWidth, implicitContentWidth + leftPadding + rightPadding)
                                        Accessible.name: modelData.label
                                        ToolTip.visible: iconOnly && (hovered || visualFocus)
                                        ToolTip.text: modelData.label
                                        onClicked: {
                                            root.controller.control(download.modelData.jobId, "", modelData.action)
                                        }
                                    }
                                }
                            }
                        }
                        // Variant choice is required only when the archive has alternatives.
                        ColumnLayout {
                            Layout.fillWidth: true
                            visible: !!download.modelData.choosing
                            Button { text: qsTr("Choose variant"); visible: root.controller.currentJobId !== download.modelData.jobId; onClicked: root.controller.selectJob(download.modelData.jobId) }
                            Repeater {
                                model: root.controller.currentJobId === download.modelData.jobId ? root.controller.candidates : []
                                CheckBox {
                                    id: choice
                                    required property var modelData
                                    Layout.fillWidth: true
                                    visible: !!modelData.available
                                    text: modelData.optionName || modelData.sourceLabel || qsTr("Variant")
                                    checked: root.controller.selectedIds.includes(modelData.candidateId || "")
                                    contentItem: Label { text: choice.text; leftPadding: 28; textFormat: Text.PlainText; wrapMode: Text.Wrap }
                                    onToggled: {
                                        let ids = root.controller.selectedIds.filter(id => id !== modelData.candidateId)
                                        if (checked) ids.push(modelData.candidateId)
                                        root.controller.selectedIds = ids
                                    }
                                }
                            }
                            Button { objectName: "addDownloadSelection"; text: qsTr("Use selected variants"); visible: root.controller.currentJobId === download.modelData.jobId; enabled: root.controller.selectedIds.length > 0; onClicked: {
                                const form = root.controller.installMetadata(download.modelData.jobId)
                                if (root.controller.replacementJob && !root.controller.automaticReplacementJob) root.controller.reviewSelection(download.modelData.jobId, root.controller.selectedIds, form)
                                else root.controller.addSelection(form)
                            } }
                            Button { text: qsTr("Save file choices"); visible: root.controller.currentJobId === download.modelData.jobId && root.controller.replacementJob && !root.controller.automaticReplacementJob && root.controller.review.phase === "ready"; enabled: root.controller.canCommit; onClicked: root.controller.commit(download.modelData.jobId) }
                        }
                    }
                }
            }
        }
    }
    Item { Layout.fillHeight: true; visible: root.empty }
}
