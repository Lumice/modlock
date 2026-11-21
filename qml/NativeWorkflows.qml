pragma ComponentBehavior: Bound
import QtQuick
import Deadlock.Native

Item {
    id: root
    required property LibraryModel library
    property string workflow: ""
    property string sourcePath: ""
    property string chosenGame: ""
    property bool automaticApplyPreview: false
    property string localJob: ""
    property string localContext: ""
    property string scheduledDraft: ""
    property bool localReviewVisible: false
    property var attemptedDrafts: ({})
    readonly property bool localPreviewAttempted: localJob !== ""
        && attemptedDrafts[localJob] === library.localImport.draftGeneration
    readonly property bool popupOpen: workflow !== "" || library.options.visible || library.options.orderVisible
        || library.options.historyVisible || library.fonts.managerVisible || library.fonts.visible || library.remote.visible
    signal downloadsHidden()
    function openDownloads() { library.remote.showDownloads() }
    function openRemote(submission) { return library.remote.resolve(submission) }
    function openReplacement(modId, action) { return library.remote.beginReplacement(modId, action) }
    property string installedError: ""
    // Set while an import started from the list is running, so the list can
    // come back (rescanned) once that import finishes.
    property bool installedFollowUp: false
    property string installedJob: ""
    function openInstalled() { workflow = "installed" }
    function openBisect() { workflow = "bisect" }
    // Close the list first so the import review can open on its own.
    function importInstalled(ids, linkIds) {
        installedError = ""
        installedJob = ""
        installedFollowUp = ids.length === 1
        workflow = ""
        if (!library.importInstalledMods(ids, linkIds)) {
            installedFollowUp = false
            installedError = library.installedModsError || qsTr("This mod can't be imported right now. Another import may still be running.")
            workflow = "installed"
        }
    }
    Connections {
        target: root.library.localImport
        function onPreparationStarted(jobId, contextId) {
            if (root.installedFollowUp && root.installedJob === "") root.installedJob = jobId
        }
        function onChanged() {
            const c = root.library.localImport
            if (!root.installedFollowUp || root.installedJob === "" || c.jobId !== root.installedJob) return
            if (!["succeeded", "failed", "cancelled", "interrupted"].includes(c.phase)) return
            root.installedFollowUp = false
            root.installedJob = ""
            if (c.phase === "succeeded") root.library.scanInstalledMods()
            Qt.callLater(() => { if (root.workflow === "") root.openInstalled() })
        }
    }
    function openImport() {
        sourcePath = library.sourcePath
        if (chosenGame === "") chosenGame = library.gamePath || library.detectedGamePath
        workflow = "import"
    }
    function openApply() {
        chosenGame = library.gamePath || chosenGame || library.detectedGamePath
        if (chosenGame !== "") {
            workflow = ""
            automaticApplyPreview = true
            if (!library.previewApply(chosenGame)) {
                automaticApplyPreview = false
            }
        } else workflow = "apply"
    }
    function openLocal() {
        const controller = library.localImport
        if (controller.jobId === "") return
        if (workflow === "local" && localJob === controller.jobId && localContext === controller.contextId) return
        if (workflow === "local") workflow = ""
        previewTimer.stop()
        scheduledDraft = ""
        localJob = controller.jobId
        localContext = controller.contextId
        localReviewVisible = false
        workflow = "local"
    }
    function openProfiles() { workflow = "profiles" }
    function openRestore() { library.previewRestore() }
    function pruneAttempts() {
        const retained = library.localImport.jobs.map(job => job.jobId)
        const next = Object.assign({}, attemptedDrafts)
        let changed = false
        for (const id of Object.keys(next)) {
            if (!retained.includes(id)) { delete next[id]; changed = true }
        }
        if (changed) attemptedDrafts = next
    }
    function syncLocalPreview() {
        const controller = library.localImport
        if (workflow === "local" && (localJob !== controller.jobId || localContext !== controller.contextId)) {
            previewTimer.stop()
            scheduledDraft = ""
            workflow = ""
            return
        }
        if (workflow !== "local" || !localReviewVisible || !controller.draftCurrent
                || controller.selectionDraft.length === 0 || controller.review.token) {
            previewTimer.stop()
            scheduledDraft = ""
            return
        }
        const generation = controller.draftGeneration
        if (attemptedDrafts[localJob] === generation) {
            previewTimer.stop()
            scheduledDraft = ""
            return
        }
        if (scheduledDraft !== generation) {
            scheduledDraft = generation
            previewTimer.restart()
        } else if (!previewTimer.running && controller.canReview) {
            dispatchLocalPreview()
        }
    }
    function dispatchLocalPreview() {
        const controller = library.localImport
        const generation = scheduledDraft
        if (workflow !== "local" || !localReviewVisible || localJob !== controller.jobId
                || localContext !== controller.contextId || !controller.draftCurrent
                || generation === "" || generation !== controller.draftGeneration
                || controller.selectionDraft.length === 0 || !controller.canReview) return
        scheduledDraft = ""
        const next = Object.assign({}, attemptedDrafts)
        next[localJob] = generation
        attemptedDrafts = next
        if (!controller.reviewCurrentDraft()) {
            const retryable = Object.assign({}, attemptedDrafts)
            if (retryable[localJob] === generation) delete retryable[localJob]
            attemptedDrafts = retryable
            scheduledDraft = generation
        }
    }
    function retryLocalPreview() {
        const controller = library.localImport
        if (workflow !== "local" || !localReviewVisible || localJob !== controller.jobId
                || localContext !== controller.contextId || !controller.draftCurrent
                || controller.selectionDraft.length === 0 || !controller.canReview) return
        previewTimer.stop()
        scheduledDraft = controller.draftGeneration
        dispatchLocalPreview()
    }
    onWorkflowChanged: {
        if (workflow === "local") return
        previewTimer.stop()
        scheduledDraft = ""
        localReviewVisible = false
        localJob = ""
        localContext = ""
    }
    Timer { id: previewTimer; interval: 250; repeat: false; onTriggered: root.dispatchLocalPreview() }
    Connections {
        target: root.library.localImport
        function onChanged() { root.pruneAttempts(); root.syncLocalPreview() }
        function onContextChanged() { root.syncLocalPreview() }
        function onCancellationRequested(jobId) {
            if (root.workflow === "local" && root.localJob === jobId) root.workflow = ""
        }
    }
    Connections {
        target: root.library
        function onAutomaticReviewRequired() {
            root.chosenGame = root.library.gamePath
            root.workflow = "apply"
        }
        function onErrorChanged() {
            if (root.automaticApplyPreview && root.library.error !== "") {
                root.automaticApplyPreview = false
                if ((root.library.error.startsWith("invalidGamePath:") || root.library.error.startsWith("gamePathMismatch:"))) {
                    root.chosenGame = ""
                    root.workflow = "apply"
                }
            }
        }
        function onReviewChanged() {
            if (root.library.applyReview.token) { root.automaticApplyPreview = false; root.chosenGame = root.library.applyReview.gamePath }
            if (root.library.importReview.token) root.workflow = "import-review"
            else if (root.library.applyReview.restoreReview) root.workflow = "restore"
            else if (root.library.applyReview.token) root.workflow = "apply-review"
            else if (root.workflow.endsWith("-review")) root.workflow = ""
        }
    }
    Loader {
        active: root.library.remote.visible
        sourceComponent: RemoteInstallDialog { library: root.library; onHidden: root.downloadsHidden() }
    }
    Loader {
        active: root.library.options.visible
        asynchronous: true
        sourceComponent: OptionsDialog { controller: root.library.options }
    }
    Loader {
        active: root.library.options.orderVisible
        asynchronous: true
        sourceComponent: OrderDialog { controller: root.library.options }
    }
    Loader {
        active: root.library.options.historyVisible
        asynchronous: true
        sourceComponent: PreparationHistory { library: root.library; onImportReviewRequested: root.openLocal() }
    }
    Loader {
        active: root.library.fonts.managerVisible
        sourceComponent: InstalledFontsDialog { controller: root.library.fonts }
    }
    Loader {
        active: root.library.fonts.visible
        sourceComponent: FontDialog { controller: root.library.fonts }
    }
    Loader {
        active: root.workflow !== ""
        sourceComponent: root.workflow === "local" ? (root.library.localImport.groupRows.length > 0 ? groupReview : localReview)
                       : root.workflow === "profiles" ? profiles
                       : root.workflow === "installed" ? installed
                       : root.workflow === "bisect" ? bisectSetup : review
    }
    Component {
        id: groupReview
        GroupImportDialog {
            controller: root.library.localImport
            onHidden: if (root.workflow === "local") root.workflow = ""
        }
    }
    Component {
        id: localReview
        LocalImportDialog {
            controller: root.library.localImport
            previewAttempted: root.localPreviewAttempted
            onRetryRequested: root.retryLocalPreview()
            onOpened: { root.localReviewVisible = true; root.syncLocalPreview() }
            onHidden: {
                root.localReviewVisible = false
                if (root.workflow === "local") root.workflow = ""
            }
        }
    }
    Component {
        id: installed
        InstalledModsDialog {
            library: root.library
            importError: root.installedError
            onClosed: { root.installedError = ""; if (root.workflow === "installed") root.workflow = "" }
            onImportRequested: (ids, linkIds) => root.importInstalled(ids, linkIds)
        }
    }
    Component {
        id: bisectSetup
        BisectSetupDialog {
            library: root.library
            onClosed: if (root.workflow === "bisect") root.workflow = ""
        }
    }
    Component {
        id: profiles
        ProfilesDialog {
            library: root.library
            onClosed: root.workflow = ""
        }
    }
    Component {
        id: review
        NativeReviewDialog {
            library: root.library
            workflow: root.workflow
            sourcePath: root.sourcePath
            gamePath: root.chosenGame
            onPreviewRequested: (source, game) => {
                root.sourcePath = source
                const importing = root.workflow === "import"
                root.workflow = ""
                if (importing) root.library.previewImport(source, game)
                else if (game !== root.library.gamePath) {
                    root.workflow = "apply"
                    root.chosenGame = game
                    root.library.useGameFolder(game)
                } else { root.automaticApplyPreview = true; root.library.previewApply(game) }
            }
            onChangeFolderRequested: {
                root.library.discardApplyReview()
                root.workflow = "apply"
            }
            onCommitRequested: () => {
                const operation = root.workflow
                if (operation === "import-review") root.library.commitImport()
                else if (operation === "apply-review") root.library.commitApply()
                else if (operation === "restore") root.library.restorePrevious()
                root.workflow = ""
            }
            onCancelled: {
                root.library.discardImportReview()
                root.library.discardApplyReview()
                root.workflow = ""
            }
        }
    }
}
