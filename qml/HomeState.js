.pragma library

function state(library) {
    if (library.loading) return "loading"
    if (library.outcomeUnknown || library.recoveryRequired) return "recovery"
    if (!library.gamePath && !library.detectedGamePath) return "setup"
    if (library.externalGameinfoChange) return "external"
    if (library.pendingStatus === "selectionRequired")
        return library.viewedProfileId === library.activeProfileId ? "choices" : "viewedChoices"
    if (library.pendingStatus === "unknown" || library.error) return "unknown"
    if (library.viewedProfileId !== library.activeProfileId) return "viewed"
    if (library.pendingChanges || library.pendingStatus === "pending" || !library.appliedProfileId
            || library.appliedProfileId !== library.viewedProfileId) return "pending"
    return library.pendingStatus === "current" ? "current" : "unknown"
}
function action(state) {
    if (state === "viewed") return qsTr("Apply…")
    if (state === "current") return qsTr("Play")
    if (state === "setup") return qsTr("Choose folder…")
    if (state === "external") return qsTr("Apply again…")
    if (state === "pending") return qsTr("Play")
    if (state === "loading") return qsTr("Loading…")
    return qsTr("Review library")
}
function sentence(state) {
    switch (state) {
    case "viewed": return qsTr("Apply the selected profile")
    case "current": return qsTr("Ready to play")
    case "pending": return qsTr("Changes waiting for your next game")
    case "external": return qsTr("Another mod manager or a game update changed the game files")
    case "setup": return qsTr("Deadlock isn't set up yet")
    case "viewedChoices": return qsTr("Choose files for the selected profile")
    case "choices": return qsTr("Choose files for some mods before playing")
    case "recovery": return qsTr("The previous change needs checking")
    case "loading": return qsTr("Loading your library…")
    default: return qsTr("Your game setup needs checking")
    }
}

function summary(mods) {
    const parts = [mods.length === 1 ? qsTr("1 mod") : qsTr("%1 mods").arg(mods.length)]
    const counts = {}
    for (const mod of mods) {
        const kind = mod.kind || ""
        if (kind && kind !== "Uncategorized") counts[kind] = (counts[kind] || 0) + 1
    }
    for (const kind of Object.keys(counts).sort()) parts.push(counts[kind] + " " + kind)
    return parts.join(" · ")
}
