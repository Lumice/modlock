pragma Singleton
import QtQuick
import QtCore

// Preferences are kept in the app's own config file.
Settings {
    location: preferencesLocation
    category: "display"
    property string colourMode: typeof preferencesDefaults === "undefined" ? "modlock" : (preferencesDefaults.colourMode ?? "modlock")
    property bool reduceMotion: typeof preferencesDefaults === "undefined" ? false : (preferencesDefaults.reduceMotion ?? false)
    // Show previews of mods marked NSFW or without a known rating without asking first.
    property bool showMaturePreviews: typeof preferencesDefaults === "undefined" ? false : (preferencesDefaults.showMaturePreviews ?? false)
    property bool hideNsfwMods: typeof preferencesDefaults === "undefined" ? true : (preferencesDefaults.hideNsfwMods ?? true)
    property bool automaticApply: typeof preferencesDefaults === "undefined" ? true : (preferencesDefaults.automaticApply ?? true)
}
