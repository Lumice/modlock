pragma Singleton
import QtQuick
import DeadlockStyle

// App-level names for the shared style tokens (see style/Tokens.qml).
QtObject {
    readonly property color window: Tokens.window
    readonly property color surface: Tokens.surface2
    readonly property color surfaceLow: Tokens.surface1
    readonly property color surfaceHigh: Tokens.surface3
    readonly property color alternate: Tokens.surface1
    readonly property color text: Tokens.text
    readonly property color secondary: Tokens.textMuted
    readonly property color faint: Tokens.textFaint
    readonly property color accent: Tokens.accent
    readonly property color accentInk: Tokens.accentInk
    readonly property color accentSoft: Tokens.accentSoft
    readonly property color selectionText: Tokens.accentText
    readonly property color rule: Tokens.border
    readonly property color ruleStrong: Tokens.borderStrong
    readonly property color overlay: Tokens.scrim
    readonly property color danger: Tokens.danger
    readonly property color dangerSoft: Tokens.dangerSoft
    readonly property color success: Tokens.success
    readonly property color successSoft: Tokens.successSoft
    readonly property color warning: Tokens.warning
    readonly property color warningSoft: Tokens.warningSoft
    readonly property int spacing4: 4
    readonly property int spacing8: 8
    readonly property int spacing12: 12
    readonly property int spacing16: 16
    readonly property int spacing24: 24
    readonly property int spacing32: 32
    readonly property int radius4: Tokens.radiusSmall
    readonly property int radius: Tokens.radius
    readonly property int radiusLarge: Tokens.radiusLarge
    readonly property real body: Tokens.body
    readonly property real small: Tokens.caption
    readonly property real title: Tokens.title
    readonly property real heading: Tokens.headline
    readonly property real display: Tokens.display
    readonly property color surfaceHover: Tokens.surface3
    readonly property color surfacePressed: Tokens.surface4
    readonly property color fieldBorder: Tokens.fieldBorder
    readonly property color brass: Tokens.brass
    readonly property color brassSoft: Tokens.brassSoft
    readonly property color live: Tokens.live
    readonly property color accentRing: Tokens.accentRing
    readonly property color focus: Tokens.focus
    readonly property int radiusCard: Tokens.radiusCard
    readonly property string displayFamily: Tokens.displayFamily
    readonly property real wordmark: Tokens.wordmark
    readonly property int spacing20: 20
    readonly property int spacing48: 48
    readonly property int motionFast: Tokens.fast
    readonly property int motionNormal: Tokens.normal
    readonly property int motionPanel: Tokens.panel
    // Placeholder values that say nothing ("General", "Uncategorized", "Unknown") are hidden.
    function meaningful(value) {
        const v = (value || "").toString().trim()
        return ["general", "uncategorized", "unknown", "other", "none"].includes(v.toLowerCase()) ? "" : v
    }
    // Error text for people: drops the leading "codeName: " the helper adds
    // and rewords the few errors a player can act on.
    function friendlyError(value) {
        const v = (value || "").toString()
        if (v === "externalGameinfoChange") return qsTr("Deadlock’s files changed. Review your setup.")
        if (v === "gameBuildChanged") return qsTr("Deadlock updated. Check your mods for fixes.")
        if (v === "performanceOutdated") return qsTr("Your game has a different version of these performance settings. To remove them, close Deadlock, choose this preset again and apply it, then choose None and apply again.")
        if (/^[a-z]+(?:[A-Z][a-zA-Z0-9]*)+$/.test(v)) return qsTr("This change couldn’t be completed. Review your setup and try again.")
        const m = /^([a-z][A-Za-z]+): ([\s\S]*)$/.exec(v)
        if (!m) return v
        // Only the native library's version refusal (older helpers send a generic line);
        // old-manager files carry their own specific unsupportedVersion messages.
        if (m[1] === "unsupportedVersion" && /native library version|newer Modlock/.test(m[2]))
            return qsTr("This library was changed by a newer Modlock. Update the app to open it.")
        if (m[1] === "performanceConfig")
            return qsTr("Couldn't change performance settings. Check the game files in Steam and try again.")
        if (m[1] === "performanceOutdated")
            return qsTr("Your game has a different version of these performance settings. To remove them, close Deadlock, choose this preset again and apply it, then choose None and apply again.")
        if (m[1] === "originalManagerTakeover")
                return qsTr("The original Deadlock Mod Manager's mods are active in citadel/addons. Import those mods first, or choose Apply again to review turning them off. Their files stay where they are; Restore puts them back.")
        if (m[1] === "unknownMount") {
            const folder = (/mount: (.+?)\. Preserve/.exec(m[2]) || [])[1] || ""
            return qsTr("Another mod manager added a mod folder to the game (%1) that this app doesn't manage. Add those mods here with Add mods → From the game folder, or turn them off in the other manager, then apply again.").arg(folder)
        }
        return m[2]
    }
    // Recovery notice: generic guidance, plus the helper's own cause when the
    // current error carries one ("Cause during <phase>: kind: text"). Any other
    // library error (an older transport failure, say) is not about this
    // recovery, so it is not shown here. Internal kind names are removed and
    // the latest blocker (after "refusal check failed") wins.
    function recoveryText(value) {
        const generic = qsTr("An earlier change didn't finish. Recover before making more changes.")
        const v = (value || "").toString()
        const at = v.lastIndexOf("Cause during ")
        if (at < 0) return generic
        let cause = v.slice(at).replace(/^Cause during [^:]*: /, "")
        const refusal = cause.lastIndexOf("; refusal check failed: ")
        if (refusal >= 0) cause = cause.slice(refusal + "; refusal check failed: ".length)
        // The cause is "kind: message"; drop only that one kind, never text inside the message.
        cause = cause.replace(/^[a-z]+[A-Z][A-Za-z0-9]*: /, "").trim()
        return cause ? generic + "\n" + cause : generic
    }
    function downloadError(value) {
        const raw = value && typeof value === "object"
            ? (value.kind ? value.kind + ": " + (value.message || "") : (value.message || ""))
            : (value || "").toString()
        const kind = (/^([a-z][A-Za-z]+):/.exec(raw) || [])[1] || ""
        switch (kind) {
        case "invalidSelection": case "needsSelection":
            return qsTr("The selected files are no longer available. Choose files again.")
        case "storageFull":
            return qsTr("Not enough disk space. Free up space and try again.")
        case "hostNotAllowed":
            return qsTr("The download server is unapproved or inactive. Try again later.")
        case "transportStorage":
            return qsTr("Could not save the download. Check the storage location and try again.")
        case "transportTimeout":
            return qsTr("The download timed out. Check your connection and try again.")
        case "transportNetwork": case "transportTls": case "providerUnavailable":
            return qsTr("Could not reach the download. Check your connection and try again.")
        case "providerNotFound":
            return qsTr("This download is no longer available.")
        case "archiveUnsupported": case "archiveRejected": case "archiveLimit": case "layoutUnsupported":
            return qsTr("Could not open these mod files. Try another download.")
        case "staleRevision": case "staleReview":
            return qsTr("The library changed. Review the download again.")
        case "transportChecksum": case "transportFraming":
            return qsTr("The download was incomplete or damaged. Try again.")
        }
        const message = friendlyError(raw)
        return /\b(sealed|durable|cohort|candidate|preparation|consent|rebase|members)\b/i.test(message)
            ? qsTr("Could not add this mod. Try again.") : message
    }
    // Human-readable size, e.g. "5.7 MB".
    function bytes(value) {
        const n = Number(value) || 0
        if (n >= 1073741824) return qsTr("%1 GB").arg((n / 1073741824).toFixed(1))
        if (n >= 1048576) return qsTr("%1 MB").arg((n / 1048576).toFixed(1))
        if (n >= 1024) return qsTr("%1 KB").arg(Math.round(n / 1024))
        return qsTr("%1 bytes").arg(n)
    }
    function icon(name) { return "qrc:/qt/qml/DeadlockStyle/icons/" + name + ".svg" }
}
