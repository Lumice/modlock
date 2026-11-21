pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import Deadlock.Native
import DeadlockStyle

ApplicationWindow {
    id: root
    Binding { target: Tokens; property: "colourMode"; value: Preferences.colourMode }
    Binding { target: Tokens; property: "reduceMotion"; value: galleryLoader.active || Preferences.reduceMotion }
    width: 1200
    height: 780
    minimumWidth: 720
    minimumHeight: 520
    visible: true
    title: qsTr("Modlock")
    color: Theme.window
    font.pointSize: Theme.body
    property alias performanceVisible: chrome.performanceVisible
    signal sourceRequested(url path)
    property bool closeWhenReady: false
    property string previousWorkflow: ""
    property string downloadsOrigin: ""
    property string newImportJob: ""
    property string newImportContext: ""
    property int autoImportSerial: 0
    function canOpenInstallLink() { return !root.closeWhenReady && !workflows.popupOpen && !launchWorkflows.popupOpen && !installLinkMessage.visible && !sourceDialog.visible && !localImportJob.pickerOpen && !chrome.popupOpen && !navigation.popupOpen }
    function installLinkError(message) { installLinkText.text = message; installLinkMessage.open() }
    Dialog {
        id: installLinkMessage
        objectName: "installLinkMessage"
        anchors.centerIn: parent
        title: qsTr("Install link")
        modal: true
        standardButtons: Dialog.Ok
        width: Math.min(480, root.width - 40)
        Label { id: installLinkText; width: parent.width; wrapMode: Text.Wrap; textFormat: Text.PlainText }
    }
    Dialog {
        objectName: "preferencesSaveNoticeDialog"
        title: qsTr("Settings")
        visible: preferencesSaveNotice.length > 0
        anchors.centerIn: parent
        standardButtons: Dialog.Ok
        width: Math.min(460, root.width - 40)
        Label { width: parent.width; text: preferencesSaveNotice; textFormat: Text.PlainText; wrapMode: Text.Wrap }
    }
    function discardAutoImport() {
        newImportJob = ""
        newImportContext = ""
        ++autoImportSerial
    }
    function canAutoOpenImport() {
        return root.visible && root.active && root.visibility !== Window.Minimized && !root.closeWhenReady
                && !sourceDialog.visible && !localImportJob.pickerOpen && !chrome.popupOpen
                && !navigation.popupOpen && !workflows.popupOpen && !launchWorkflows.popupOpen
    }
    function checkNewImport() {
        if (newImportJob === "") return
        const controller = libraryModel.localImport
        if (controller.jobId !== newImportJob || controller.contextId !== newImportContext || controller.cancelPending
                || ["cancelRequested", "cancelled", "interrupted", "failed", "succeeded", "recoveryRequired"].includes(controller.phase)) {
            discardAutoImport()
            return
        }
        if (controller.phase === "ready" && !canAutoOpenImport()) { discardAutoImport(); return }
        if (!controller.presentationReady) return
        const job = newImportJob
        const context = newImportContext
        discardAutoImport()
        if (!controller.draftCurrent || !canAutoOpenImport()) return
        const serial = autoImportSerial
        Qt.callLater(() => {
            if (root.autoImportSerial !== serial || controller.jobId !== job || controller.contextId !== context
                    || !controller.presentationReady || !controller.draftCurrent || !root.canAutoOpenImport()) return
            workflows.openLocal()
        })
    }
    function canReturnHeaderFocus() {
        return root.visible && !root.closeWhenReady && !workflows.popupOpen && !sourceDialog.visible && !localImportJob.pickerOpen
                && !launchWorkflows.popupOpen
                && !libraryModel.options.visible && !libraryModel.options.orderVisible && !libraryModel.options.historyVisible
                && !libraryModel.fonts.visible && !libraryModel.fonts.managerVisible && !navigation.popupOpen && !chrome.popupOpen
    }
    function canReturnModFontFocus() {
        return root.visible && root.active && root.visibility !== Window.Minimized && !root.closeWhenReady
                && !workflows.popupOpen && !sourceDialog.visible && !localImportJob.pickerOpen && !chrome.popupOpen
                && !launchWorkflows.popupOpen
                && !libraryModel.options.visible && !libraryModel.options.orderVisible && !libraryModel.options.historyVisible
                && !libraryModel.fonts.visible && !libraryModel.fonts.managerVisible
    }
    // Development aid for --screenshot-page: put the UI into a known state.
    property var screenshotShareDialog: null
    property string screenshotShareSource: ""
    Timer {
        id: screenshotShareTimer
        interval: 100; repeat: true
        onTriggered: {
            if (libraryModel.loading || libraryModel.busy || !libraryModel.viewedProfileId) return
            stop()
            const component = Qt.createComponent(root.screenshotShareSource)
            root.screenshotShareDialog = component.createObject(root, root.screenshotShareSource === "MaintenanceDialog.qml" ? {library: libraryModel} : {library: libraryModel, profileId: libraryModel.viewedProfileId})
        }
    }
    Loader {
        id: galleryLoader
        parent: Overlay.overlay
        active: false
        anchors.fill: parent
        z: 1000
        sourceComponent: Component { ControlGallery { library: libraryModel } }
    }
    function screenshotSetup(page) {
        if (page === "gallery") {
            galleryLoader.active = true
            width = galleryLoader.item.implicitWidth
            height = galleryLoader.item.implicitHeight
        }
        else if (page === "maintenance") {
            screenshotShareSource = "MaintenanceDialog.qml"
            screenshotShareTimer.start()
        }
        else if (page === "share-export" || page.startsWith("share-export=")) {
            screenshotShareSource = page === "share-export" ? "ProfileShareDialog.qml" : page.slice(13)
            screenshotShareTimer.start()
        }
        else if (page === "home") navigation.showHome()
        else if (page === "library") navigation.showLibrary()
        else if (page === "grid") { navigation.showLibrary(); navigation.libraryPresentationMode = "grid" }
        else if (page === "skins") navigation.showSkins("")
        else if (page === "skins-abrams") { navigation.browsePresentationMode = "list"; navigation.showSkins("Abrams") }
        else if (page === "browse") navigation.showBrowse()
        else if (page === "settings") navigation.showSettings()
        else if (page === "bulk") { navigation.showLibrary(); navigation.markForScreenshot() }
        else if (page === "bulk-off") { navigation.showLibrary(); navigation.bulkOffForScreenshot() }
        else if (page === "collections") { navigation.showBrowse(); libraryModel.catalog.updateQuery({kind: "collections"}) }
        else if (page === "collection-item") { navigation.showBrowse(); libraryModel.catalog.updateQuery({kind: "collections"}); screenshotItemTimer.start() }
        else if (page === "browse-item") { navigation.showBrowse(); screenshotItemTimer.start() }
        else if (page === "select") { navigation.showLibrary(); libraryModel.selectedId = libraryModel.idAt(1) }
        else if (page === "narrow") root.width = 820
        else if (page === "wide") root.width = 1400
        else if (page === "profiles") workflows.openProfiles()
        else if (page === "downloads") navigation.showDownloads()
        else if (page === "options") libraryModel.options.open(libraryModel.selectedId)
        else if (page === "order") libraryModel.options.openOrder()
        else if (page === "install") { navigation.showBrowse(); screenshotInstallTimer.start() }
        else if (page === "apply") workflows.openApply()
        else if (page === "launch-settings") launchWorkflows.settingsOpen = true
        else if (page === "more") chrome.openMoreForScreenshot()
        else if (page === "installed") workflows.openInstalled()
        else if (page === "bisect-setup") workflows.openBisect()
        else if (page === "tweaks") navigation.showTweaks()
        else if (page === "crosshair") { navigation.showTweaks(); screenshotTabTimer.tab = 1; screenshotTabTimer.start() }
        else if (page === "crosshair-review") { navigation.showTweaks(); screenshotTabTimer.tab = 1; screenshotTabTimer.review = true; screenshotTabTimer.start() }
        else if (page === "autoexec") { navigation.showTweaks(); screenshotTabTimer.tab = 2; screenshotTabTimer.start() }
        else if (page === "tweaks-keep") { navigation.showTweaks(); screenshotKeepTimer.start() }
        else if (page === "group") screenshotGroupTimer.start()
        else if (page === "modpage") {
            libraryModel.selectedId = libraryModel.idAt(0)
            const page = libraryModel.selectedMod.remoteSubmission
            if (page) libraryModel.catalog.openSubmission(page, true)
        }
    }
    Timer { id: screenshotInstallTimer; interval: 2500; onTriggered: workflows.openRemote(libraryModel.catalog.model.item(libraryModel.catalog.model.identityAt(3)).submission) }
    // Waits for the installed-mod scan, then reviews the first three rows together.
    Timer {
        id: screenshotGroupTimer; interval: 1000; repeat: true
        onTriggered: {
            if (libraryModel.installedMods.length === 0) { if (!libraryModel.installedModsBusy) libraryModel.scanInstalledMods(); return }
            if (libraryModel.installedModsBusy || !libraryModel.localImport.canPrepare) return
            stop()
            const rows = libraryModel.installedMods.filter(r => !r.alreadyInLibrary && !r.error).slice(0, 3)
            workflows.importInstalled(rows.map(r => r.id), rows.filter(r => r.suggestedMatch && r.suggestedMatch.link).map(r => r.id))
        }
    }
    Timer {
        id: screenshotTabTimer; property int tab; property bool review: false; interval: 2500
        onTriggered: {
            navigation.tweaksTab(tab)
            // Preview only: the review is computed but never committed.
            if (review) { libraryModel.crosshair.setValue("color.g", 200); navigation.tweaksReview(libraryModel.crosshair) }
        }
    }
    Timer { id: screenshotKeepTimer; interval: 3000; onTriggered: libraryModel.performance.select(libraryModel.performance.detected.presetId || "") }
    Timer { id: screenshotItemTimer; interval: 2500; onTriggered: libraryModel.catalog.openItem(libraryModel.catalog.model.identityAt(3)) }
    onClosing: close => {
        root.discardAutoImport()
        if (!root.closeWhenReady && !launchWorkflows.requestWindowClose()) {
            close.accepted = false
            return
        }
        if (!libraryModel.shutdownReady) {
            close.accepted = false
            closeWhenReady = true
            libraryModel.requestShutdown()
        }
    }
    onActiveChanged: if (!root.active && newImportJob !== "" && libraryModel.localImport.phase === "ready") discardAutoImport()
    onVisibilityChanged: if ((!root.visible || root.visibility === Window.Minimized) && newImportJob !== "" && libraryModel.localImport.phase === "ready") discardAutoImport()
    Connections {
        target: libraryModel
        function onShutdownChanged() {
            if (root.closeWhenReady && libraryModel.shutdownReady) Qt.callLater(root.close)
        }
    }
    Connections {
        target: libraryModel.localImport
        function onPreparationStarted(jobId, contextId) {
            root.discardAutoImport()
            root.newImportJob = jobId
            root.newImportContext = contextId
            root.checkNewImport()
        }
        function onCancellationRequested(jobId) {
            if (jobId === libraryModel.localImport.jobId) root.discardAutoImport()
        }
        function onContextChanged() { root.checkNewImport() }
        function onChanged() { root.checkNewImport() }
    }

    LibraryModel {
        id: libraryModel
        objectName: "libraryModel"
        automaticApply: Preferences.automaticApply
        autoApplyWorkflowOpen: workflows.popupOpen || launchWorkflows.popupOpen || installLinkMessage.visible
                               || sourceDialog.visible || localImportJob.pickerOpen || chrome.popupOpen || navigation.popupOpen
    }
    FrameMetrics {
        id: frameMetrics
        window: root
        collecting: chrome.performanceVisible && root.visible && root.visibility !== Window.Minimized
    }

    header: AppHeader {
        id: chrome
        library: libraryModel
        focusReturnAllowed: root.canReturnHeaderFocus()
        downloadsAllowed: !navigation.popupOpen && !workflows.popupOpen && !launchWorkflows.popupOpen && !sourceDialog.visible && !localImportJob.pickerOpen
        onOpenSourceRequested: sourceDialog.open()
        onImportRequested: workflows.openImport()
        onRestoreRequested: workflows.openRestore()
        onProfilesRequested: workflows.openProfiles()
        onApplyRequested: workflows.openApply()
        onLaunchSettingsRequested: launchWorkflows.settingsOpen = true
        onLaunchDetailsRequested: launchWorkflows.detailsOpen = true
        onDownloadsRequested: { root.downloadsOrigin = "more"; navigation.showDownloads() }
    }
    NativeWorkflows {
        id: workflows; library: libraryModel
        onDownloadsHidden: Qt.callLater(() => {
            if (root.downloadsOrigin === "library") {
                if (root.canReturnModFontFocus()) navigation.focusLibraryDownload()
                root.downloadsOrigin = ""
                return
            }
            if (!root.canReturnHeaderFocus()) return
            if (root.downloadsOrigin === "catalog") navigation.focusDownload()
            else chrome.focusMore()
            root.downloadsOrigin = ""
        })
    }
    Connections {
        target: workflows
        function onWorkflowChanged() {
            const prior = root.previousWorkflow
            root.previousWorkflow = workflows.workflow
            if (workflows.workflow !== "") { root.discardAutoImport(); return }
            const profile = prior === "profiles"
            if (!profile && !["apply", "apply-review", "restore", "import", "import-review"].includes(prior)) return
            Qt.callLater(() => {
                if (!root.canReturnHeaderFocus()) return
                if (profile) chrome.focusProfiles()
                else chrome.focusMore()
            })
        }
    }
    LaunchWorkflows {
        onFindBrokenMod: workflows.openBisect()
        id: launchWorkflows
        library: libraryModel
        closing: root.closeWhenReady
        onWindowCloseReady: Qt.callLater(root.close)
        onReviewDismissed: Qt.callLater(() => { if (root.canReturnHeaderFocus()) { chrome.focusLaunch() } })
        onDetailsDismissed: Qt.callLater(() => { if (root.canReturnHeaderFocus()) { chrome.focusLaunch() } })
        onSettingsDismissed: Qt.callLater(() => chrome.focusLaunch())
    }
    FileDialog {
        id: sourceDialog
        title: qsTr("Choose the Deadlock Mod Manager library file (state.json)")
        nameFilters: [qsTr("JSON files (*.json)"), qsTr("All files (*)")]
        onAccepted: { root.sourceRequested(selectedFile); Qt.callLater(() => { if (root.canReturnHeaderFocus()) chrome.focusMore() }) }
        onRejected: Qt.callLater(() => { if (root.canReturnHeaderFocus()) chrome.focusMore() })
    }

    DropArea {
        objectName: "localDropArea"
        anchors.fill: parent
        onEntered: drag => { drag.accepted = drag.hasUrls }
        onDropped: drop => { drop.accepted = drop.hasUrls && libraryModel.localImport.prepareDrop(drop.urls) }
    }
    Binding { target: libraryModel.localImport; property: "visible"; value: root.visible && root.visibility !== Window.Minimized }
    Label {
        visible: root.closeWhenReady
        z: 10
        anchors.bottom: parent.bottom
        text: libraryModel.remote.retiring ? qsTr("Waiting for downloads and recovery to stop safely…") : qsTr("Closing after current work finishes…")
        padding: 8
    }
    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        LaunchNotice {
            canReviewApply: libraryModel.canPreviewApply
            id: launchNotice
            Layout.fillWidth: true
            controller: libraryModel.launch
            occupied: libraryModel.outcomeUnknown || (libraryModel.recoveryNeedsAttention && (!launchNotice.needsClear || libraryModel.canRecover))
            onApplyRequested: workflows.openApply()
            onDetailsRequested: launchWorkflows.detailsOpen = true
        }
        StatusNotice {
            id: statusNotice
            Layout.fillWidth: true
            library: libraryModel
            occupied: launchNotice.visible
            importActive: localImportJob.hasNotice
            showChecks: navigation.destination === "library"
            onApplyRequested: workflows.openApply()
            onInstalledRequested: workflows.openInstalled()
        }
        Callout {
            id: helperStopNotice
            objectName: "helperStopNotice"
            Layout.fillWidth: true
            Layout.margins: Theme.spacing8
            visible: libraryModel.helperStopped
            tone: "danger"
            title: qsTr("Modlock helper stopped")
            text: {
                const cause = Theme.friendlyError(libraryModel.stopReason) || libraryModel.stopReason
                return (cause ? cause + "\n" : "") + qsTr("Reload the library or restart Modlock to reconnect.")
            }
            Button {
                objectName: "helperStopReloadButton"
                visible: !libraryModel.demoMode
                enabled: libraryModel.canRefresh && !libraryModel.loading && !libraryModel.busy
                text: qsTr("Reload")
                onClicked: libraryModel.refresh()
            }
        }
        Pane {
            Layout.fillWidth: true
            visible: !launchNotice.visible && !statusNotice.visible && !localImportJob.hasNotice && !!libraryModel.fonts.offer.modId
            GridLayout {
                anchors.fill: parent
                columns: root.width < 850 ? 1 : 3
                Label {
                    Layout.fillWidth: true
                    text: qsTr("Optional game fonts for %1").arg(libraryModel.fonts.offer.title || "")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                Button { text: qsTr("Install fonts…"); enabled: libraryModel.fonts.available; onClicked: libraryModel.fonts.openOffer() }
                Button { text: qsTr("Not now"); onClicked: libraryModel.fonts.dismissOffer() }
            }
        }
        LocalImportJob {
            id: localImportJob
            Layout.fillWidth: true
            controller: libraryModel.localImport
            occupied: launchNotice.visible || statusNotice.visible
            onReviewRequested: { root.discardAutoImport(); workflows.openLocal() }
        }
        AppNavigation {
            id: navigation
            globalExternalNoticeVisible: statusNotice.externalChangeShown
            Layout.fillWidth: true
            Layout.fillHeight: true
            library: libraryModel
            fontFocusReturnAllowed: root.canReturnModFontFocus()
            downloadsAvailable: libraryModel.remote.available && !workflows.popupOpen && !launchWorkflows.popupOpen
                                && !sourceDialog.visible && !localImportJob.pickerOpen
            onDownloadsRequested: { root.downloadsOrigin = "more"; navigation.showDownloads() }
            onApplyRequested: workflows.openApply()
            onSettingsAction: action => {
                if (action === "launchSettings") launchWorkflows.settingsOpen = true
                else if (action === "apply") workflows.openApply()
                else if (action === "restore") workflows.openRestore()
                else if (action === "order") libraryModel.options.openOrder()
                else if (action === "history") libraryModel.options.showHistory()
                else if (action === "source") sourceDialog.open()
                else if (action === "profiles") workflows.openProfiles()
                else if (action === "bisect") workflows.openBisect()
            }
            onImportFolderRequested: localImportJob.openFolder()
            onUpstreamImportRequested: workflows.openImport()
            onInstalledRequested: workflows.openInstalled()
            onArtworkRequested: localImportJob.chooseArtwork()
            onClearArtworkRequested: localImportJob.clearArtwork()
            pendingArtwork: localImportJob.artwork
            onImportFilesRequested: localImportJob.openFiles()
            onBrowseRequested: navigation.showBrowse()
            onDownloadRequested: submission => {
                if (!root.canAutoOpenImport()) return
                root.downloadsOrigin = "catalog"
                if (!workflows.openRemote(submission)) root.downloadsOrigin = ""
            }
            onReplacementRequested: (modId, action) => {
                root.downloadsOrigin = "library"
                if (!workflows.openReplacement(modId, action)) root.downloadsOrigin = ""
            }
        }
        Loader {
            Layout.fillWidth: true
            active: chrome.performanceVisible
            visible: active
            sourceComponent: Component {
                PerformancePanel { metrics: frameMetrics }
            }
        }
    }
}

