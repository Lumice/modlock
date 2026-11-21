.pragma library
// Player wording follows the GPL-3.0 Deadlock Mod Manager modHealth strings.
function label(status) {
    return ({fixable: qsTr("Can be fixed"), partlyFixable: qsTr("Partly fixable"), fixed: qsTr("Fixed for this patch"), partlyFixed: qsTr("Partly fixed"), needsUpdate: qsTr("Needs an update"), originalFiles: qsTr("Original files")})[status] || ""
}
function symptoms(values) {
    const names = {cameraAim: qsTr("Camera and aim"), animations: qsTr("Animations and movement"), appearance: qsTr("Colours and textures"), model: qsTr("Model"), gameData: qsTr("Abilities, stats and text"), crashRisk: qsTr("Model data issues")}
    return (values || []).map(s => names[s] || qsTr("Other issues")).join(", ")
}
function summary(health) {
    const fixes = symptoms(health.fixSymptoms)
    const remaining = symptoms(health.remainingSymptoms)
    switch (health.status) {
    case "fixable": return qsTr("Made for an older Deadlock version. These parts can be fixed for this patch: %1.").arg(fixes)
    case "partlyFixable": return qsTr("Can fix: %1. Still needs an update from the author: %2.").arg(fixes).arg(remaining)
    case "fixed": return qsTr("Fixed for this patch: %1.").arg(fixes)
    case "partlyFixed": return qsTr("Fixed: %1. Still needs an update from the author: %2.").arg(fixes).arg(remaining)
    case "originalFiles": return qsTr("Using the mod's original files. Can fix: %1.").arg(fixes) + (remaining ? qsTr(" Still needs an update: %1.").arg(remaining) : "")
    case "needsUpdate": return qsTr("These parts need an update from the mod author: %1.").arg(symptoms(health.symptoms))
    default: return ""
    }
}
