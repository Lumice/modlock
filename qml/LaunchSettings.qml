pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Dialog {
    id: root
    required property LibraryModel library
    property bool closing: false
    property bool windowClosePrompt: false
    readonly property LaunchController controller: library.launch
    property string pinnedContext: ""
    property string pinnedRoot: ""
    property bool initialized: false
    property string baseline: ""
    property string baselineRevision: ""
    property string draft: ""
    property string readRequestId: ""
    property string readPurpose: ""
    property string readDraftStamp: ""
    property string saveAttemptId: ""
    property string submittedRevision: ""
    property string message: ""
    property string confirmationPurpose: ""
    property string confirmationStamp: ""
    readonly property bool dirty: initialized && draft !== baseline
    readonly property string draftStamp: JSON.stringify([pinnedContext, baselineRevision, baseline, draft])
    readonly property bool contextCurrent: initialized && pinnedContext === controller.settingsContextId
        && pinnedRoot === library.dataDir && library.nativeMode && !library.demoMode
    readonly property var readState: controller.settingsReadState
    readonly property var saveState: controller.settingsSaveState
    readonly property bool currentSave: saveState.contextId === controller.settingsContextId
    readonly property bool saveActive: currentSave && ["opening", "saving"].includes(saveState.status)
    readonly property bool saveUnknown: currentSave && ["unconfirmed", "refused"].includes(saveState.status)
    readonly property bool popupOpen: visible || confirmation.visible
    readonly property bool readPending: !!readRequestId
    readonly property bool canSave: dirty && contextCurrent && !closing && !readPending && !saveActive && !saveUnknown
        && controller.available && !controller.busy && !controller.writeGate && !controller.settingsSavePending
        && !library.busy && !library.outcomeUnknown && !library.recoveryRequired
        && controller.settingsSnapshot.contextId === pinnedContext
        && controller.settingsSnapshot.revision === baselineRevision && controller.settingsRevision === baselineRevision
    signal dismissed()

    function applySnapshot(snapshot) {
        initialized = true
        pinnedContext = snapshot.contextId
        pinnedRoot = library.dataDir
        baseline = snapshot.additionalArguments
        baselineRevision = snapshot.revision
        draft = baseline
        message = currentSave && saveState.status === "confirmed" ? qsTr("Saved.")
            : currentSave && saveState.status === "failed" ? saveState.error : ""
    }
    function discardDraft() {
        initialized = false
        pinnedContext = ""
        pinnedRoot = ""
        baseline = ""
        baselineRevision = ""
        draft = ""
        readRequestId = ""
        readPurpose = ""
        saveAttemptId = ""
        submittedRevision = ""
        message = ""
    }
    function openEditor() {
        if (closing) return
        open()
        if (!initialized) {
            const snapshot = controller.settingsSnapshot
            if (snapshot.contextId === controller.settingsContextId && snapshot.revision === controller.settingsRevision)
                applySnapshot(snapshot)
        }
        if (!initialized || (contextCurrent && !dirty)) requestRead("initial")
        Qt.callLater(() => {
            if (!root.visible || root.popupConfirmation) return
            if (argumentEditor.enabled) argumentEditor.forceActiveFocus()
            else cancelButton.forceActiveFocus()
        })
    }
    readonly property bool popupConfirmation: confirmation.visible || windowClosePrompt
    function requestRead(purpose) {
        if (closing || readPending) return
        const before = controller.settingsReadState
        const context = controller.settingsContextId
        let state = before
        if (!(state.contextId === context && state.status === "reading")) {
            const accepted = controller.readSettings()
            state = controller.settingsReadState
            if ((!accepted || state.requestId === before.requestId) && !(state.contextId === context && state.status === "reading")) {
                message = state.error || qsTr("Couldn't start reading saved arguments. Try again when the manager is ready.")
                return
            }
        }
        if (state.contextId !== context || !state.requestId) return
        readRequestId = state.requestId
        readPurpose = purpose
        readDraftStamp = draftStamp
        observeRead()
    }
    function observeRead() {
        if (!readRequestId || readState.requestId !== readRequestId) return
        if (readState.contextId !== controller.settingsContextId) return
        if (readState.status === "succeeded") {
            const snapshot = controller.settingsSnapshot
            if (snapshot.contextId !== readState.contextId || snapshot.serial !== readState.snapshotSerial) return
            if (readPurpose === "reload" && readDraftStamp === draftStamp && (!initialized || contextCurrent)) applySnapshot(snapshot)
            else if (readPurpose === "initial" && (!initialized || (contextCurrent && !dirty))) applySnapshot(snapshot)
            else if (readPurpose === "reload") message = qsTr("The draft changed while reading. Review it before reloading.")
            readRequestId = ""
            readPurpose = ""
        } else if (readState.status === "failed" || readState.status === "unconfirmed") {
            message = readState.error || qsTr("Couldn't read saved arguments. Reconnect or try again.")
            readRequestId = ""
            readPurpose = ""
        }
    }
    function observeSnapshot() {
        const snapshot = controller.settingsSnapshot
        if (!initialized && !readPending && snapshot.contextId === controller.settingsContextId
                && snapshot.revision === controller.settingsRevision) applySnapshot(snapshot)
        else if (initialized && contextCurrent && !dirty && !readPending && !saveActive && !saveUnknown
                 && !controller.settingsSavePending && snapshot.contextId === pinnedContext)
            applySnapshot(snapshot)
    }
    function observeSave() {
        if (!saveAttemptId || saveState.contextId !== pinnedContext || saveState.attemptId !== saveAttemptId) return
        if (saveState.status === "confirmed" && saveState.operationId
                && saveState.expectedRevision === submittedRevision && saveState.savedRevision) {
            const snapshot = controller.settingsSnapshot
            if (snapshot.contextId !== pinnedContext || snapshot.revision !== saveState.savedRevision) return
            applySnapshot(snapshot)
            saveAttemptId = ""
            message = qsTr("Saved.")
        } else if (saveState.status === "failed") {
            const snapshot = controller.settingsSnapshot
            if (saveState.operationId && snapshot.contextId === pinnedContext && snapshot.revision === controller.settingsRevision)
                applySnapshot(snapshot)
            saveAttemptId = ""
            message = saveState.error
        } else if (saveUnknown && saveState.error) {
            message = saveState.error
        }
    }
    function save() {
        if (!canSave) return
        submittedRevision = baselineRevision
        const previous = controller.settingsSaveState.attemptId
        const accepted = controller.saveSettings(draft)
        const state = controller.settingsSaveState
        if (accepted && state.contextId === pinnedContext && state.attemptId && state.attemptId !== previous) {
            saveAttemptId = state.attemptId
            message = ""
            observeSave()
        } else {
            message = qsTr("Couldn't start saving. Check the text and the current library state.")
        }
    }
    function confirmAction(purpose) {
        confirmationPurpose = purpose
        confirmationStamp = draftStamp
        confirmation.open()
    }
    function requestClose() {
        if (closing) return
        if (dirty) confirmAction("close")
        else { discardDraft(); close() }
    }
    function requestReload() {
        if (closing) return
        if (dirty) confirmAction("reload")
        else requestRead("reload")
    }
    function finishConfirmation() {
        if (dirty && confirmationStamp !== draftStamp) {
            Qt.callLater(() => root.confirmAction(root.confirmationPurpose))
            return
        }
        if (confirmationPurpose === "close") { discardDraft(); close() }
        else if (confirmationPurpose === "reload") requestRead("reload")
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(620, parent ? parent.width - 32 : 620)
    height: Math.min(implicitHeight, parent ? parent.height - 32 : implicitHeight)
    title: qsTr("Launch options")
    modal: true
    Overlay.modal: Rectangle { color: Theme.overlay }
    closePolicy: Popup.NoAutoClose
    onClosed: dismissed()
    Shortcut {
        sequences: [StandardKey.Cancel]
        enabled: root.visible && !root.popupConfirmation && !root.closing
        onActivated: root.requestClose()
    }
    Connections {
        target: root.controller
        function onSettingsSnapshotChanged() { root.observeSnapshot() }
        function onSettingsReadStateChanged() { root.observeRead() }
        function onSettingsSaveStateChanged() { root.observeSave() }
        function onSettingsContextChanged() {
            root.readRequestId = ""
            root.readPurpose = ""
            if (root.initialized && !root.contextCurrent)
                root.message = qsTr("The library changed. This draft belongs to %1. Discard it before editing the current library.").arg(root.pinnedRoot)
        }
    }
    contentItem: ColumnLayout {
        spacing: Theme.spacing8
        Label {
            Layout.fillWidth: true
            text: qsTr("Extra options for Deadlock when you press Play, for example -novid. Used exactly as typed.")
            color: Theme.secondary
            wrapMode: Text.Wrap
        }
        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.max(60, argumentEditor.font.pixelSize * 3 + 24)
            TextArea {
                id: argumentEditor
                objectName: "launchArgumentsEditor"
                placeholderText: qsTr("For example, -novid")
                text: root.draft
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.WrapAnywhere
                selectByMouse: true
                enabled: root.initialized && root.contextCurrent && !root.closing && !root.readPending
                    && !root.saveActive && !root.saveUnknown && !root.controller.settingsSavePending
                Accessible.name: qsTr("Additional Steam launch arguments")
                onTextChanged: if (root.draft !== text) { root.draft = text; root.message = "" }
            }
        }
        Label {
            Layout.fillWidth: true
            text: !root.initialized ? (root.readPending ? qsTr("Reading saved arguments…") : qsTr("Saved arguments are unavailable."))
                : !root.contextCurrent ? qsTr("Draft belongs to a different library.")
                : root.saveActive ? qsTr("Saving arguments…")
                : root.saveUnknown || root.controller.settingsSavePending ? qsTr("The previous save still needs checking.")
                : root.controller.settingsRevision !== root.baselineRevision ? qsTr("Saved arguments changed. Reload before saving.")
                : root.dirty ? qsTr("Unsaved changes") : ""
            visible: text !== ""
            color: Theme.secondary
            font.pointSize: Theme.small
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        ScrollView {
            id: errorView
            Layout.fillWidth: true
            Layout.preferredHeight: visible ? Math.min(80, errorText.implicitHeight) : 0
            visible: !!root.message || !!root.saveState.error || (root.saveState.status !== "confirmed" && !!root.controller.settingsError)
            clip: true
            contentWidth: availableWidth
            Label {
                id: errorText
                objectName: "launchSettingsError"
                width: errorView.availableWidth
                text: Theme.friendlyError(root.message || root.saveState.error || (root.saveState.status !== "confirmed" ? root.controller.settingsError : ""))
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: Theme.spacing8
            Button {
                text: qsTr("Discard changes")
                flat: true
                visible: root.dirty || !root.initialized || !root.contextCurrent
                enabled: !root.closing && !root.readPending && !root.saveActive && !root.saveUnknown && !root.controller.settingsSavePending
                    && root.controller.available && (!root.controller.busy || root.readState.status === "reading")
                    && (!root.initialized || root.contextCurrent)
                onClicked: root.requestReload()
            }
            Button {
                text: root.controller.settingsSavePending ? qsTr("Check save result") : qsTr("Read saved arguments")
                visible: root.controller.settingsSavePending || (!!root.controller.settingsError && root.controller.writeGate)
                enabled: !root.closing && !root.controller.busy
                onClicked: if (!root.controller.reconcileSettings()) root.message = qsTr("Couldn't read saved arguments yet. Reconnect or try again when the manager is ready.")
            }
            Button {
                text: qsTr("Close to reconnect")
                visible: root.saveActive || root.saveUnknown || root.readState.status === "unconfirmed" || root.controller.settingsSavePending || !root.controller.available
                    || (root.currentSave && root.saveState.status === "failed" && root.controller.writeGate)
                enabled: !root.closing
                ToolTip.visible: hovered || activeFocus
                ToolTip.text: qsTr("Keeps this draft. A save already started may still finish.")
                onClicked: root.close()
            }
            Button {
                id: diagnostics
                visible: !!root.controller.error
                text: checked ? qsTr("Hide details") : qsTr("Show details")
                checkable: true
            }
        }
        ScrollView {
            id: diagnosticsView
            Layout.fillWidth: true
            Layout.preferredHeight: visible ? Math.min(80, diagnosticsText.implicitHeight) : 0
            visible: diagnostics.visible && diagnostics.checked
            clip: true
            contentWidth: availableWidth
            Label {
                id: diagnosticsText
                width: diagnosticsView.availableWidth
                text: root.controller.error
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
        }
    }
    footer: DialogButtonBox {
        Button { id: cancelButton; text: qsTr("Cancel"); enabled: !root.closing; DialogButtonBox.buttonRole: DialogButtonBox.ActionRole; onClicked: root.requestClose() }
        Button { objectName: "saveLaunchSettingsButton"; highlighted: true; Component.onCompleted: highlighted = true; text: qsTr("Save"); enabled: root.canSave; DialogButtonBox.buttonRole: DialogButtonBox.ActionRole; onClicked: root.save() }
    }
    Dialog {
        id: confirmation
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(420, parent ? parent.width - 32 : 420)
        modal: true
        Overlay.modal: Rectangle { color: Theme.overlay }
        title: qsTr("Discard unsaved changes?")
        standardButtons: Dialog.Ok | Dialog.Cancel
        Component.onCompleted: standardButton(Dialog.Ok).text = qsTr("Discard")
        onOpened: standardButton(Dialog.Cancel).forceActiveFocus()
        contentItem: Label {
            text: root.saveActive || root.saveUnknown || root.controller.settingsSavePending
                ? qsTr("Discard this draft? A save already started may still finish; closing does not cancel it.")
                : root.confirmationPurpose === "reload" ? qsTr("Discard this draft and load saved arguments?") : qsTr("Discard this draft?")
            wrapMode: Text.Wrap
        }
        onAccepted: root.finishConfirmation()
        onRejected: if (root.visible) { if (argumentEditor.enabled) argumentEditor.forceActiveFocus(); else cancelButton.forceActiveFocus() }
    }
}
