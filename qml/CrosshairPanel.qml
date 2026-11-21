pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

// Saved crosshair editor. The preview is a static approximation; see geometry comments below.
ColumnLayout {
    id: root
    required property GameConfigController controller
    signal reviewRequested()
    readonly property var v: {
        const base = controller.values || ({})
        const override = (base.overrides || {})[hero] || {}
        return Object.assign({}, base, override, {
            color: Object.assign({r:255,g:255,b:255}, base.color || {}, override.color || {}),
            outlineColor: Object.assign({r:0,g:0,b:0}, base.outlineColor || {}, override.outlineColor || {})
        })
    }
    objectName: "crosshairPanel"
    property int zoom: 1
    readonly property var ranges: controller.ranges || ({})
    readonly property var color: v.color || ({ r: 255, g: 255, b: 255 })
    // The old manager's crosshair lines in autoexec.cfg run at game start and override these.
    property bool overriddenByAutoexec: false
    property string hero: ""
    onHeroChanged: controller.editHero = hero
    Component.onCompleted: controller.editHero = hero
    property string codeMessage: ""
    property bool customColourExpanded: false
    spacing: Theme.spacing12

    function range(key, fallback) { return ranges[key] || fallback }
    function set(key, value) { controller.setValue((hero ? "overrides." + hero + "." : "") + key, value) }
    function hex() {
        const h = n => Math.round(Number(n) || 0).toString(16).padStart(2, "0")
        return "#" + h(color.r) + h(color.g) + h(color.b)
    }
    function setHex(value) {
        if (!/^#[0-9a-fA-F]{6}$/.test(value)) return
        root.set("color.r", parseInt(value.slice(1, 3), 16))
        root.set("color.g", parseInt(value.slice(3, 5), 16))
        root.set("color.b", parseInt(value.slice(5, 7), 16))
    }
    function copyText(text) { clip.text = text; clip.selectAll(); clip.copy() }
    TextEdit { id: clip; visible: false }

    component ValueSlider: RowLayout {
        id: row
        objectName: "crosshairValue." + key
        property string key
        property string label
        property real value
        property var r: ({ min: 0, max: 100, step: 1 })
        property int decimals: 0
        Layout.fillWidth: true
        spacing: Theme.spacing8
        Label { text: row.label; Layout.preferredWidth: 160; color: Theme.secondary; wrapMode: Text.Wrap }
        Slider {
            id: slider
            Layout.fillWidth: true
            background: Rectangle {
                x: slider.leftPadding; y: slider.topPadding + (slider.availableHeight - height) / 2
                width: slider.availableWidth; height: 4; radius: 2; color: Theme.fieldBorder
            }
            handle: Rectangle {
                x: slider.leftPadding + slider.visualPosition * (slider.availableWidth - width)
                y: slider.topPadding + (slider.availableHeight - height) / 2
                width: 16; height: 16; radius: 8
                color: Theme.surface; border.width: 2; border.color: Theme.accentRing
                Rectangle { anchors.fill: parent; anchors.margins: -3; radius: 11; color: "transparent"; border.width: 2; border.color: Theme.focus; visible: slider.visualFocus }
            }
            from: row.r.min; to: row.r.max; stepSize: row.r.step
            value: row.value
            enabled: root.controller.available && !root.controller.busy
            Accessible.name: row.label
            onMoved: root.set(row.key, Number(value.toFixed(row.decimals)))
        }
        Label { objectName: "crosshairNumber." + row.key; text: Number(row.value).toFixed(row.decimals); Layout.preferredWidth: 44; horizontalAlignment: Text.AlignRight; font.features: ({ "tnum": 1 }) }
    }

    Label {
        Layout.fillWidth: true
        text: qsTr("Changes the crosshair for all heroes, or the selected hero. Review changes before saving. A backup lets you undo the last save. Close Deadlock first.")
        color: Theme.secondary
        wrapMode: Text.Wrap
    }
    Rectangle {
        Layout.fillWidth: true
        visible: root.overriddenByAutoexec
        implicitHeight: overrideText.implicitHeight + Theme.spacing24
        radius: Theme.radius
        color: Theme.warningSoft
        Label {
            id: overrideText
            x: Theme.spacing12; y: Theme.spacing12
            width: parent.width - Theme.spacing24
            text: qsTr("Your autoexec.cfg still has a crosshair from the old Mod Manager, and it overrides the one shown here when the game starts. Saving here removes that old part.")
            wrapMode: Text.Wrap
        }
    }
    GridLayout {
        Layout.fillWidth: true
        columns: root.Window.width < 900 ? 1 : 2
        columnSpacing: Theme.spacing16
        rowSpacing: Theme.spacing16
        // Live preview
        ColumnLayout {
            Layout.alignment: Qt.AlignTop
            spacing: Theme.spacing8
            Rectangle {
                Layout.preferredWidth: 300
                Layout.preferredHeight: 300
                radius: Theme.radius
                clip: true
                gradient: Gradient {
                    GradientStop { position: 0; color: "#5b6b7a" }
                    GradientStop { position: 0.55; color: "#3a4048" }
                    GradientStop { position: 1; color: "#23272c" }
                }
                Canvas {
                    id: canvas
                    anchors.fill: parent
                    readonly property var deps: [root.v, root.hero, root.zoom, Screen.devicePixelRatio]
                    onDepsChanged: requestPaint()
                    onPaint: {
                        const ctx = getContext("2d")
                        ctx.reset()
                        const v = root.v
                        // Schema: width/height and outline widths/gaps are independent.
                        // Installed popup_settings.xml calls dot_size pixels, while the
                        // runtime schema calls it radius. Diameter interpretation and
                        // square dot are inferred, pending a captured game comparison.
                        // No guessed per-hero weapon spread: show the static custom shape.
                        // Canvas coordinates are logical pixels. Keep one game
                        // pixel equal to one physical pixel at 1x, also at fractional DPI.
                        const dpr = Screen.devicePixelRatio
                        ctx.translate(Math.floor(width * dpr / 2) / dpr, Math.floor(height * dpr / 2) / dpr)
                        ctx.scale(root.zoom / dpr, root.zoom / dpr)
                        const rgb = (c, a) => "rgba(" + c.r + "," + c.g + "," + c.b + "," + a + ")"
                        const fill = v.color || {r:255,g:255,b:255}
                        const outline = v.outlineColor || {r:0,g:0,b:0}
                        const rect = (x,y,w,h,colour) => { ctx.fillStyle = colour; ctx.fillRect(x,y,w,h) }
                        const box = (x,y,w,h,border,gap,opacity,outlineOpacity) => {
                            if (w <= 0 || h <= 0) return
                            x = Math.round(x); y = Math.round(y)
                            if (border > 0 && outlineOpacity > 0) {
                                ctx.fillStyle = rgb(outline,outlineOpacity)
                                // Four strips keep the outline gap transparent.
                                const d = border + gap
                                rect(x-d,y-d,w+2*d,border,ctx.fillStyle)
                                rect(x-d,y+h+gap,w+2*d,border,ctx.fillStyle)
                                rect(x-d,y-gap,border,h+2*gap,ctx.fillStyle)
                                rect(x+w+gap,y-gap,border,h+2*gap,ctx.fillStyle)
                            }
                            rect(x,y,w,h,rgb(fill,opacity))
                        }
                        const dot = Number(v.dotSize ?? 2)
                        box(-dot/2,-dot/2,dot,dot,Number(v.dotOutlineWidth || 0),Number(v.dotOutlineGap || 0),Number(v.dotOpacity ?? 1),Number(v.dotOutlineOpacity ?? 1))
                        const w = Number(v.width || 0), h = Number(v.height || 0)
                        // Inferred: gap measures the distance from centre to inner edge.
                        const gap = Number(v.gap || 0)
                        for (const r of [[-w/2,gap,w,h],[-w/2,-gap-h,w,h],[gap,-w/2,h,w],[-gap-h,-w/2,h,w]])
                            box(...r,Number(v.pipOutlineWidth || 0),Number(v.pipOutlineGap || 0),Number(v.pipOpacity ?? 1),Number(v.pipOutlineOpacity ?? 1))
                    }
                }
            }
            RowLayout {
                Label { text: qsTr("Preview as"); color: Theme.secondary }
                ComboBox {
                    objectName: "crosshairHero"
                    Layout.fillWidth: true
                    textRole: "name"
                    model: root.controller.heroScopes.length ? root.controller.heroScopes : [{name: qsTr("All heroes"), key: ""}]
                    onActivated: root.hero = model[currentIndex].key
                    Accessible.name: qsTr("Preview hero")
                }
                ComboBox {
                    objectName: "crosshairZoom"
                    model: ["1×", "2×", "4×", "8×"]
                    onActivated: root.zoom = [1,2,4,8][currentIndex]
                    Accessible.name: qsTr("Preview zoom")
                }
            }
            Label {
                Layout.preferredWidth: 300
                objectName: "crosshairPreviewNote"
                text: qsTr("Approximate static preview. Shared dot/pip colours; no themed artwork or weapon spread.")
                color: Theme.faint
                font.pointSize: Theme.small
                wrapMode: Text.Wrap
            }
        }
        // Controls
        ColumnLayout {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignTop
            spacing: Theme.spacing8
            Label { text: qsTr("Pips"); font.weight: Font.DemiBold }
            ValueSlider { key: "gap"; label: qsTr("Gap"); value: root.v.gap || 0; r: root.range("gap", { min: -20, max: 50, step: 1 }) }
            ValueSlider { key: "width"; label: qsTr("Thickness"); value: root.v.width || 0; r: root.range("width", { min: 0, max: 100, step: 0.1 }); decimals: 1 }
            ValueSlider { key: "height"; label: qsTr("Length"); value: root.v.height || 0; r: root.range("height", { min: 0, max: 100, step: 0.1 }); decimals: 1 }
            ValueSlider { key: "pipOpacity"; label: qsTr("Opacity"); value: root.v.pipOpacity ?? 1; r: root.range("pipOpacity", { min: 0, max: 1, step: 0.1 }); decimals: 2 }
            Label { text: qsTr("Outline"); font.weight: Font.DemiBold }
            ValueSlider { key: "pipOutlineWidth"; label: qsTr("Pip outline width"); value: root.v.pipOutlineWidth || 0; r: root.range("pipOutlineWidth", {min:0,max:10,step:1}) }
            ValueSlider { key: "pipOutlineGap"; label: qsTr("Pip outline gap"); value: root.v.pipOutlineGap || 0; r: root.range("pipOutlineGap", {min:0,max:100,step:1}) }
            ValueSlider { key: "pipOutlineOpacity"; label: qsTr("Pip outline opacity"); value: root.v.pipOutlineOpacity ?? 1; r: root.range("pipOutlineOpacity", {min:0,max:1,step:0.01}); decimals: 2 }
            ValueSlider { key: "dotOutlineWidth"; label: qsTr("Dot outline width"); value: root.v.dotOutlineWidth || 0; r: root.range("dotOutlineWidth", {min:0,max:10,step:1}) }
            ValueSlider { key: "dotOutlineGap"; label: qsTr("Dot outline gap"); value: root.v.dotOutlineGap || 0; r: root.range("dotOutlineGap", {min:0,max:100,step:1}) }
            Label { text: qsTr("Behaviour"); font.weight: Font.DemiBold }
            Switch { objectName: "crosshairToggle.pipGapStatic"; text: qsTr("Fixed gap (doesn't widen)"); checked: !!root.v.pipGapStatic; onToggled: root.set("pipGapStatic", checked) }
            Switch { objectName: "crosshairToggle.gapCollapse"; text: qsTr("Close gap when firing"); checked: !!root.v.gapCollapse; onToggled: root.set("gapCollapse", checked) }
            Switch { objectName: "crosshairToggle.themed"; text: qsTr("Use hero-themed crosshair"); checked: !!root.v.themed; onToggled: root.set("themed", checked) }
            Label { text: qsTr("Per-hero"); font.weight: Font.DemiBold }
            Label { Layout.fillWidth: true; text: root.hero ? qsTr("Edits apply to this hero. Other values inherit from All heroes.") : qsTr("Select a hero to edit its overrides."); wrapMode: Text.Wrap; color: Theme.secondary }
            Button {
                text: qsTr("Use all-hero settings")
                enabled: root.hero !== "" && !root.controller.busy
                onClicked: root.controller.clearHeroOverride(root.hero)
            }
            Label { text: qsTr("Dot"); font.weight: Font.DemiBold; Layout.topMargin: Theme.spacing8 }
            ValueSlider { key: "dotSize"; label: qsTr("Dot size (pixels)"); value: root.v.dotSize ?? 2; r: root.range("dotSize", {min:1,max:100,step:1}) }
            ValueSlider { key: "dotOpacity"; label: qsTr("Dot"); value: root.v.dotOpacity ?? 1; r: root.range("dotOpacity", { min: 0, max: 1, step: 0.1 }); decimals: 2 }
            ValueSlider { key: "dotOutlineOpacity"; label: qsTr("Dot outline"); value: root.v.dotOutlineOpacity ?? 1; r: root.range("dotOutlineOpacity", { min: 0, max: 1, step: 0.1 }); decimals: 2 }
            Label { text: qsTr("Colour"); font.weight: Font.DemiBold; Layout.topMargin: Theme.spacing8 }
            RowLayout {
                spacing: Theme.spacing8
                Repeater {
                    model: ["#ffffff", "#00ff00", "#00ffff", "#ffff00", "#ff0000", "#ff00ff", root.hex()]
                    delegate: Button {
                        required property string modelData
                        required property int index
                        Layout.preferredWidth: 30
                        Layout.preferredHeight: 30
                        enabled: root.controller.available && !root.controller.busy
                        Accessible.name: index === 6 ? qsTr("Current colour %1").arg(modelData) : qsTr("Use colour %1").arg(modelData)
                        ToolTip.visible: hovered
                        ToolTip.text: Accessible.name
                        background: Rectangle {
                            radius: Theme.radius
                            color: parent.modelData
                            border.width: parent.activeFocus || parent.hovered ? 3 : 1
                            border.color: parent.activeFocus ? Theme.accent : Theme.ruleStrong
                        }
                        onClicked: root.setHex(modelData)
                    }
                }
                TextField {
                    id: hexField
                    objectName: "crosshairHex"
                    Layout.preferredWidth: 110
                    text: root.hex()
                    maximumLength: 7
                    validator: RegularExpressionValidator { regularExpression: /^#[0-9a-fA-F]{6}$/ }
                    Accessible.name: qsTr("Crosshair colour in hexadecimal")
                    placeholderText: "#rrggbb"
                    selectByMouse: true
                    enabled: root.controller.available && !root.controller.busy
                    onTextEdited: if (acceptableInput) root.setHex(text)
                    onEditingFinished: text = Qt.binding(() => root.hex())
                }
            }
            ValueSlider { key: "outlineColor.r"; label: qsTr("Outline red"); value: root.v.outlineColor?.r || 0; r: ({min:0,max:255,step:1}) }
            ValueSlider { key: "outlineColor.g"; label: qsTr("Outline green"); value: root.v.outlineColor?.g || 0; r: ({min:0,max:255,step:1}) }
            ValueSlider { key: "outlineColor.b"; label: qsTr("Outline blue"); value: root.v.outlineColor?.b || 0; r: ({min:0,max:255,step:1}) }
            Button {
                text: qsTr("Custom")
                flat: true
                icon.source: Theme.icon(root.customColourExpanded ? "chevron-down" : "chevron-right")
                onClicked: root.customColourExpanded = !root.customColourExpanded
            }
            ColumnLayout {
                Layout.fillWidth: true
                visible: root.customColourExpanded
                ValueSlider { key: "color.r"; label: qsTr("Red"); value: root.color.r || 0; r: { "min": 0, "max": 255, "step": 1 } }
                ValueSlider { key: "color.g"; label: qsTr("Green"); value: root.color.g || 0; r: { "min": 0, "max": 255, "step": 1 } }
                ValueSlider { key: "color.b"; label: qsTr("Blue"); value: root.color.b || 0; r: { "min": 0, "max": 255, "step": 1 } }
            }
        }
    }
    // Share codes
    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacing8
        TextField {
            id: codeField
            Layout.fillWidth: true
            placeholderText: qsTr("Paste a legacy crosshair code (basic settings only)")
            selectByMouse: true
        }
        Button {
            text: qsTr("Load code")
            enabled: codeField.text.trim() !== "" && !root.controller.busy
            onClicked: root.codeMessage = root.controller.importCode(codeField.text.trim()) ? "" : qsTr("That code isn't valid.")
        }
        Button {
            text: qsTr("Copy legacy code")
            flat: true
            onClicked: { root.copyText(root.controller.exportCode()); root.codeMessage = qsTr("Copied.") }
        }
    }
    Label { visible: root.codeMessage !== ""; text: root.codeMessage; color: Theme.secondary; font.pointSize: Theme.small }
    Label {
        Layout.fillWidth: true
        visible: root.controller.error !== ""
        text: root.controller.error
        color: Theme.danger
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }
    RowLayout {
        Layout.fillWidth: true
        Button {
            text: qsTr("Reset editor defaults")
            flat: true
            enabled: !root.controller.busy && !!root.controller.defaults
            onClicked: {
                const d = root.controller.defaults
                for (const k of Object.keys(d)) {
                    if (k === "color") { root.set("color.r", d.color.r); root.set("color.g", d.color.g); root.set("color.b", d.color.b) }
                    else if (k === "outlineColor") { for (const c of ["r","g","b"]) root.set("outlineColor." + c, d[k][c]) }
                    else if (k !== "hero" && k !== "overrides") root.set(k, d[k])
                }
            }
        }
        Button { text: qsTr("Discard changes"); flat: true; enabled: !root.controller.busy; onClicked: root.controller.refresh() }
        Item { Layout.fillWidth: true }
        Button {
            text: qsTr("Undo last save…")
            icon.source: Theme.icon("undo")
            visible: root.controller.canUndo
            enabled: !root.controller.busy
            onClicked: if (root.controller.undo()) root.reviewRequested()
        }
        Button {
            objectName: "crosshairSave"
            highlighted: true
            text: qsTr("Save to game…")
            icon.source: Theme.icon("check")
            enabled: root.controller.available && !root.controller.busy
            onClicked: if (root.controller.preview()) root.reviewRequested()
        }
    }
}
