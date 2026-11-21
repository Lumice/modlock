.pragma library

function phase(value) {
    switch (value) {
    case "idle": return qsTr("Ready to launch")
    case "reviewing": return qsTr("Checking setup…")
    case "ready": return qsTr("Review preparation changes")
    case "preparing": return qsTr("Preparing game files…")
    case "verifyingPrepared": return qsTr("Checking game files…")
    case "openingSteam": return qsTr("Opening Steam…")
    case "waitingForGame": return qsTr("Waiting for Steam to start Deadlock…")
    case "running": return qsTr("Deadlock is running")
    case "cancelled": return qsTr("Launch waiting cancelled")
    case "failed": return qsTr("Launch failed")
    case "timedOut": return qsTr("Stopped waiting: startup time limit reached")
    case "completed": return qsTr("Observed game session ended")
    case "resolved": return qsTr("Preparation hold resolved")
    case "reconciling": return qsTr("Checking previous launch…")
    case "interrupted": return qsTr("Waiting for Steam to start Deadlock…")
    default: return qsTr("Launch state unavailable")
    }
}

function progress(value) {
    if (value.kind === "repairing") return qsTr("Preparing fixes — %1 of %2 mods checked. This may take a moment.").arg(value.checked || 0).arg(value.total || 0)
    switch (value.kind) {
    case "copying": return qsTr("Copied %1 of %2 files; %3 of %4 bytes")
        .arg(value.filesDone).arg(value.filesTotal).arg(value.bytesDone).arg(value.bytesTotal)
    case "verifying": return qsTr("Checking game file contents")
    case "committing": return qsTr("Finishing preparation")
    default: return ""
    }
}

function preparation(value) {
    switch (value) {
    case "reviewing": return qsTr("Checking preparation")
    case "reviewed": return qsTr("Preparation reviewed")
    case "executing": return qsTr("Preparation in progress")
    case "prepared": return qsTr("Files prepared")
    case "notConfirmed": return qsTr("Prepared files not confirmed")
    default: return qsTr("No current preparation observation")
    }
}

function handoff(value) {
    switch (value) {
    case "notDispatched": return qsTr("Steam request not dispatched")
    case "armed": return qsTr("Prepared for Steam handoff")
    case "dispatched": return qsTr("Launch request dispatched to Steam")
    case "spawnFailed": return qsTr("Could not open the Steam handler")
    case "uncertain": return qsTr("Steam handoff is uncertain")
    case "settled": return qsTr("Steam handoff hold settled")
    default: return qsTr("No current Steam handoff observation")
    }
}

function observation(value) {
    switch (value) {
    case "notStarted": return qsTr("Game waiting has not started")
    case "waiting": return qsTr("Waiting to observe Deadlock")
    case "running": return qsTr("Deadlock observed running")
    case "unavailable": return qsTr("Game observation unavailable")
    case "retired": return qsTr("Game waiting stopped")
    case "exited": return qsTr("Observed Deadlock process exited")
    default: return qsTr("No current game waiting observation")
    }
}

function game(value) {
    switch (value.state) {
    case "idle": return qsTr("Last game check: no Deadlock process found")
    case "running": return qsTr("Last game check: Deadlock running")
    default: return qsTr("Game check unavailable; idle is not confirmed")
    }
}

function stop(value) {
    switch (value.status) {
    case "stopping": return qsTr("Stop requested; waiting for exit confirmation")
    case "alreadyExited": return qsTr("Selected game process had already exited")
    case "unavailable": return qsTr("Stop unavailable for the selected game process")
    case "exited": return qsTr("Selected game process exit confirmed")
    case "stillRunning": return qsTr("Selected game process still running after the stop wait")
    default: return ""
    }
}

function stopAvailability(value) {
    switch (value) {
    case "available": return qsTr("Stop available with a fresh game check")
    case "differentUser": return qsTr("Stop unavailable: game belongs to another user")
    case "pidfdUnsupported": return qsTr("Stop unavailable: kernel process handles unsupported")
    case "pidfdUnavailable": return qsTr("Stop unavailable: process handle could not be opened")
    default: return qsTr("Stop unavailable: a fresh eligible game check is required")
    }
}

function number(value) { return Number(value).toLocaleString(Qt.locale(), "f", 0) }

function mode(value) {
    switch (value) {
    case "vanilla": return qsTr("Vanilla: zero managed addon payloads")
    case "modded": return qsTr("Modded launch")
    default: return qsTr("Launch mode unavailable")
    }
}

function identity(ready, profiles) {
    const summary = ready.summary || {}
    const method = (ready.preparationInput || {}).method
    const mode = summary.mode || (method === "native.vanilla" ? "vanilla" : method === "native.apply" ? "modded" : "")
    const profileId = summary.profileId || ""
    const profile = profiles.find(row => row.id === profileId)
    return {mode: mode, profileName: summary.profileName || (profile || {}).name || profileId}
}

function summaryRows(summary) {
    let rows = []
    if (summary.summaryVersion === 2)
        rows.push(qsTr("%1 directory VPKs, %2 companion files, %3 addon folders")
            .arg(number(summary.directoryCount)).arg(number(summary.companionFileCount)).arg(number(summary.shardCount)))
    for (const shard of summary.shards || [])
        rows.push(qsTr("Addon folder %1: %2 of 99 directory VPKs").arg(number(shard.index)).arg(number(shard.directoryCount)))
    for (const mount of summary.replacedMounts || []) rows.push(qsTr("Remove mount: %1").arg(mount))
    for (const mount of summary.installedMounts || []) rows.push(qsTr("Install mount: %1").arg(mount))
    for (const mount of summary.retainedMounts || []) rows.push(qsTr("Retain mount: %1").arg(mount))
    for (const warning of summary.warnings || [])
        if (typeof warning.message === "string" && warning.message) rows.push(warning.message)
    return rows
}

// Performance change lines for Apply and launch reviews: empty unless a chosen
// preset rewrites the performance config, then one "Name: old → new" per value.
function performance(perf) {
    if (!perf || !["replace", "remove"].includes(perf.overlayAction)) return ""
    const shown = v => v === null || v === undefined ? qsTr("not set") : v
    return (perf.overlayAction === "remove" ? qsTr("Performance: use Valve’s settings.") : qsTr("Performance config: sets “%1”.").arg(perf.presetName))
        + (perf.changes || []).map(c => "\n" + qsTr("%1: %2 → %3").arg(c.name).arg(shown(c.from)).arg(shown(c.to))).join("")
}

function legacyMessage(seconds) {
    const date = seconds ? new Date(Number(seconds) * 1000).toLocaleDateString() : qsTr("an unknown date")
    return qsTr("An earlier launch from %1 was never confirmed. If Deadlock isn't starting, clear it.").arg(date)
}
