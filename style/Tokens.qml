pragma Singleton
import QtQuick

// After Hours palettes, with an optional desktop-derived colour mode.
QtObject {
    id: tokens
    readonly property SystemPalette palette: SystemPalette { colorGroup: SystemPalette.Active }
    readonly property SystemPalette disabledPalette: SystemPalette { colorGroup: SystemPalette.Disabled }

    function mix(a, b, t) {
        return Qt.rgba(a.r * (1 - t) + b.r * t, a.g * (1 - t) + b.g * t, a.b * (1 - t) + b.b * t, 1)
    }
    function luminance(c) { return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b }

    property string colourMode: "modlock"
    property bool reduceMotion: false
    readonly property bool brand: colourMode !== "desktop"
    readonly property color window: brand ? (dark ? "#0e0c15" : "#f1eff5") : (palette.window)
    readonly property color text: brand ? (dark ? "#eee8dc" : "#1b1626") : (palette.windowText)
    readonly property bool dark: luminance(palette.window) < 0.5

    // Surfaces: each step moves slightly toward the text colour.
    readonly property color surface0: window
    readonly property color surface1: brand ? (dark ? "#14111d" : "#e9e6ef") : (mix(window, text, dark ? 0.035 : 0.025))
    readonly property color surface2: brand ? (dark ? "#1a1626" : "#fbfafd") : (mix(window, text, dark ? 0.065 : 0.045))
    readonly property color surface3: brand ? (dark ? "#231e32" : "#ebe7f1") : (mix(window, text, dark ? 0.10 : 0.075))
    readonly property color surface4: brand ? (dark ? "#2c263e" : "#d8d3e1") : (mix(window, text, dark ? 0.145 : 0.11))
    readonly property color field: brand ? (dark ? "#120f1b" : "#ffffff") : (mix(window, text, dark ? 0.05 : 0.0))

    readonly property color border: brand ? (dark ? "#2a2438" : "#dcd7e4") : (Qt.alpha(text, dark ? 0.10 : 0.13))
    readonly property color borderStrong: brand ? (dark ? "#3a3350" : "#c9c2d5") : (Qt.alpha(text, dark ? 0.20 : 0.24))

    readonly property color textMuted: brand ? (dark ? "#b4adc2" : "#4c4560") : (mix(window, text, 0.70))
    readonly property color textFaint: brand ? readableOn((dark ? "#8f879f" : "#655e76"), [window, surface1, surface2, surface3, mix(window, accent, .28)]) : (readableOn(mix(window, text, 0.55), [window, surface1, surface2, surface3]))
    readonly property color textDisabled: mix(window, text, 0.38)

    readonly property color accent: brand ? (dark ? "#7b5cc8" : "#6543b5") : (palette.highlight)
    readonly property color accentText: brand ? (dark ? "#ffffff" : "#ffffff") : (palette.highlightedText)
    readonly property color accentHover: brand ? (dark ? "#6d4fbb" : "#5a3aa6") : (dark ? Qt.lighter(accent, 1.12) : Qt.darker(accent, 1.08))
    readonly property color accentPressed: brand ? (dark ? "#5f43a6" : "#4e3191") : (dark ? Qt.darker(accent, 1.1) : Qt.darker(accent, 1.18))
    readonly property color accentSoft: Qt.alpha(accent, dark ? 0.20 : (brand ? 0.13 : 0.14))
    // Accent strong enough to read as text on every surface it's drawn on
    // (window, cards, pressed and checked-hover backgrounds). Blend toward
    // whichever of white or black reads better, stopping at the first 10%
    // step that reaches WCAG 4.5:1; the pure endpoint is the last resort.
    function channel(v) { return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }
    function relativeLuminance(c) { return 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b) }
    function contrast(a, b) {
        const x = relativeLuminance(a), y = relativeLuminance(b)
        return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)
    }
    function worstContrast(c, backgrounds) {
        let worst = 21
        for (let i = 0; i < backgrounds.length; ++i) worst = Math.min(worst, contrast(c, backgrounds[i]))
        return worst
    }
    function readableOn(c, backgrounds, minimum = 4.5) {
        c = Qt.color(c)
        if (worstContrast(c, backgrounds) >= minimum) return c
        const white = Qt.rgba(1, 1, 1, 1), black = Qt.rgba(0, 0, 0, 1)
        const target = worstContrast(white, backgrounds) >= worstContrast(black, backgrounds) ? white : black
        for (let step = 1; step < 10; ++step) {
            const x = mix(c, target, step / 10)
            if (worstContrast(x, backgrounds) >= minimum) return x
        }
        return target
    }
    readonly property color accentInk: brand ? (dark ? "#b8a1f0" : "#5a3aa6") : readableOn(accent, [window, surface1, surface2, surface3, surface4,
                                                           mix(window, accent, 0.28)])

    readonly property color danger: brand ? readableOn((dark ? "#f2707a" : "#b02a33"), [window, surface1, surface2, surface3, mix(window, accent, .28)]) : (dark ? "#f2555a" : "#c9302c")
    readonly property color dangerSoft: Qt.alpha(danger, brand ? (dark ? 0.14 : 0.12) : 0.16)
    readonly property color success: brand ? readableOn((dark ? "#5fcf98" : "#186a3c"), [window, surface1, surface2, surface3, mix(window, accent, .28)]) : (dark ? "#4cc38a" : "#1f8a4c")
    readonly property color successSoft: Qt.alpha(success, brand ? (dark ? 0.14 : 0.12) : 0.16)
    readonly property color warning: brand ? readableOn((dark ? "#f1a04e" : "#8a4c00"), [window, surface1, surface2, surface3, mix(window, accent, .28)]) : (dark ? "#f1b84b" : "#a3670b")
    readonly property color warningSoft: Qt.alpha(warning, brand ? (dark ? 0.14 : 0.12) : 0.16)

    readonly property color scrim: brand ? (dark ? Qt.rgba(6/255,5/255,10/255,.62) : Qt.rgba(27/255,22/255,38/255,.38)) : Qt.rgba(0, 0, 0, dark ? 0.55 : 0.35)
    readonly property color shadow: brand ? (dark ? Qt.rgba(0,0,0,.55) : Qt.rgba(27/255,22/255,38/255,.16)) : Qt.rgba(0, 0, 0, dark ? 0.5 : 0.18)
    readonly property color focus: accentInk

    readonly property color fieldBorder: brand ? (dark ? "#6c6382" : "#8b8299") : readableOn(mix(window, text, .45), [window, field, surface2], 3)
    readonly property color accentRing: readableOn(accent, [window, surface2], 3)
    readonly property color brass: readableOn(dark ? "#d2ae68" : "#735619", [window, surface1, surface2, surface3, mix(window, accent, .28)])
    readonly property color brassSoft: Qt.alpha(brand && !dark ? "#8a6822" : brass, dark ? .14 : .13)
    readonly property color live: readableOn(dark ? "#45d3de" : "#0d6f78", [window, surface1, surface2, surface3, mix(window, accent, .28)])
    readonly property FontLoader displayFont: FontLoader { source: "fonts/ArchivoNarrow[wght].ttf" }
    readonly property string displayFamily: displayFont.status === FontLoader.Ready ? displayFont.name : ""

    // Geometry
    readonly property int radiusSmall: 4
    readonly property int radius: 6
    readonly property int radiusLarge: 14
    readonly property int controlHeight: 34
    readonly property int controlPadding: 12

    // Type scale (major third, base = system body size)
    readonly property FontMetrics metrics: FontMetrics { }
    readonly property real body: metrics.font.pointSize > 0 ? Math.max(10, metrics.font.pointSize) : 11
    readonly property real caption: Math.max(9, body * 0.9)
    readonly property real title: body * 1.25
    readonly property real headline: body * 1.75
    readonly property real display: body * 2.25

    readonly property int radiusCard: 10
    readonly property real wordmark: body * 1.3

    // Motion
    readonly property int panel: reduceMotion ? 0 : 260
    readonly property int fast: reduceMotion ? 0 : 120
    readonly property int normal: reduceMotion ? 0 : 200
}
