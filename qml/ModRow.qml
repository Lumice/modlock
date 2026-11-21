import "HealthText.js" as HealthText
import QtQuick
import Deadlock.Native

MediaCard {
    id: root
    objectName: "modRow"
    required property string modId
    required property string modTitle
    required property string heroName
    required property string modKind
    required property bool modEnabled
    required property bool readOnly
    required property string modAuthor
    required property string modStatus
    required property string modPreview
    required property bool modPreviewHidden
    required property bool modAppliedEnabled
    required property bool modSelectionReady
    required property string modPendingStatus
    required property bool modPendingApply
    required property bool modSelectionSaved
    required property bool modAvailable
    required property string modUnavailableReason
    required property string modUpdateStatus
    updateLabel: modUpdateStatus === "available" ? qsTr("Update available") : ""
    required property string modHealthStatus
    healthLabel: HealthText.label(modHealthStatus)
    healthTone: ["fixable", "partlyFixable", "partlyFixed", "needsUpdate"].includes(modHealthStatus) ? "warning" : "muted"
    property bool nativeMode: false
    property bool canSetEnabled: false

    anchorIdentity: modId
    heading: modTitle
    subtitle: [Theme.meaningful(modAuthor), Theme.meaningful(heroName), Theme.meaningful(modKind)].filter(part => !!part).join(" · ")
    readonly property string nativeState: modPendingStatus === "selectionRequired" ? "choose"
                                        : modPendingStatus === "unknown" ? "check"
                                        : modPendingApply ? "pending"
                                        : modAppliedEnabled ? "applied" : modEnabled ? "saved" : "off"
    status: !modAvailable ? qsTr("Unavailable")
          : nativeMode ? (nativeState === "choose" ? qsTr("Choose files")
                        : nativeState === "check" ? qsTr("Needs check")
                        : "")
          : ["installed", "enabled", "disabled", "active", ""].includes(modStatus.toLowerCase()) ? "" : modStatus
    statusTone: !modAvailable ? "danger"
              : nativeMode ? (nativeState === "choose" || nativeState === "check" ? "warning"
                            : nativeState === "pending" ? "accent"
                            : nativeState === "applied" ? "success" : "muted")
              : modEnabled ? "success" : "muted"
    actionKind: "switch"
    actionEnabled: !readOnly && (!nativeMode || (canSetEnabled && (modSelectionReady || modEnabled)))
    actionChecked: modEnabled
    property string profileName: qsTr("Default")
    actionName: (modEnabled ? qsTr("%1, on in %2") : qsTr("%1, off in %2")).arg(modTitle).arg(profileName)
}
