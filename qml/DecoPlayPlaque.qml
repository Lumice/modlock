pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Shapes
import DeadlockStyle

Item {
    id: root
    property real chamfer: 10
    property bool chamferLeft: true
    property bool chamferRight: true
    property bool activeState: true
    property bool hovered: false
    property bool down: false
    property bool sheenActive: false

    Accessible.ignored: true

    readonly property real cL: chamferLeft ? Math.max(0, Math.min(chamfer, Math.floor(height / 2) - 1, Math.floor(width / 2) - 2)) : 0
    readonly property real cR: chamferRight ? Math.max(0, Math.min(chamfer, Math.floor(height / 2) - 1, Math.floor(width / 2) - 2)) : 0
    readonly property real innerInsetY: 3
    readonly property real innerInsetL: chamferLeft ? 3 : 2
    readonly property real innerInsetR: chamferRight ? 3 : 2
    readonly property real innerOffsetL: chamferLeft ? cL + innerInsetY * (Math.SQRT2 - 1) : 2
    readonly property real innerOffsetR: chamferRight ? cR + innerInsetY * (Math.SQRT2 - 1) : 2

    readonly property color baseFill: !activeState
        ? Theme.surfaceHigh
        : down ? Tokens.accentPressed
        : hovered ? Tokens.accentHover
        : Theme.accent
    readonly property color topFill: baseFill
    readonly property color bottomFill: !activeState
        ? Theme.surfaceHigh
        : (Tokens.relativeLuminance(Theme.selectionText) > 0.5 ? Qt.darker(baseFill, 1.18) : Tokens.mix(baseFill, "#ffffff", 0.14))

    readonly property color outerBorderColor: activeState
        ? Qt.alpha(Theme.brass, (hovered || down) ? 0.92 : 0.72)
        : Qt.alpha(Theme.ruleStrong, 0.75)
    readonly property color innerHairlineColor: activeState
        ? Qt.alpha(Tokens.dark ? "#e8cd96" : "#f6e5be", (hovered || down) ? 0.68 : 0.52)
        : Qt.alpha(Theme.brass, 0.24)

    readonly property real sheenLeft: (chamferLeft ? cL : 0) + 2
    readonly property real sheenRight: (chamferRight ? cR : 0) + 2
    readonly property bool shouldAnimateSheen: sheenActive
        && activeState
        && visible
        && !!Window.window
        && Window.window.visible
        && Window.visibility !== Window.Minimized
        && !Tokens.reduceMotion
        && Tokens.panel > 0
        && sheenClip.width > 12

    function stopIgnition() {
        ignitionAnim.stop()
        ignitionBar.x = -ignitionBar.width
        ignitionBar.opacity = 0
    }
    function ignite() {
        if (Tokens.reduceMotion || Tokens.panel <= 0 || !visible || !Window.window
                || !Window.window.visible || Window.visibility === Window.Minimized || sheenClip.width <= 12) {
            stopIgnition()
            return
        }
        ignitionAnim.restart()
    }
    onVisibleChanged: if (!visible) stopIgnition()
    Connections {
        target: Tokens
        function onReduceMotionChanged() { if (Tokens.reduceMotion) root.stopIgnition() }
    }

    Shape {
        anchors.fill: parent
        visible: root.width > 8 && root.height > 8
        Accessible.ignored: true

        ShapePath {
            strokeWidth: 1
            strokeColor: root.outerBorderColor
            joinStyle: ShapePath.BevelJoin
            fillGradient: LinearGradient {
                x1: 0
                y1: 0
                x2: 0
                y2: Math.max(1, root.height)
                GradientStop { position: 0.0; color: root.topFill }
                GradientStop { position: 1.0; color: root.bottomFill }
            }
            startX: root.cL + 0.5
            startY: 0.5
            PathLine { x: Math.max(root.cL + 1, root.width - root.cR - 0.5); y: 0.5 }
            PathLine { x: root.width - 0.5; y: root.cR + 0.5 }
            PathLine { x: root.width - 0.5; y: Math.max(root.cR + 1, root.height - root.cR - 0.5) }
            PathLine { x: Math.max(root.cL + 1, root.width - root.cR - 0.5); y: root.height - 0.5 }
            PathLine { x: root.cL + 0.5; y: root.height - 0.5 }
            PathLine { x: 0.5; y: Math.max(root.cL + 1, root.height - root.cL - 0.5) }
            PathLine { x: 0.5; y: root.cL + 0.5 }
            PathLine { x: root.cL + 0.5; y: 0.5 }
        }

        ShapePath {
            strokeWidth: 1
            strokeColor: root.innerHairlineColor
            fillColor: "transparent"
            joinStyle: ShapePath.BevelJoin
            startX: root.innerOffsetL
            startY: root.innerInsetY + 0.5
            PathLine { x: Math.max(root.innerOffsetL + 1, root.width - root.innerOffsetR); y: root.innerInsetY + 0.5 }
            PathLine { x: root.width - root.innerInsetR - 0.5; y: root.chamferRight ? root.innerOffsetR : root.innerInsetY + 0.5 }
            PathLine { x: root.width - root.innerInsetR - 0.5; y: Math.max(root.innerInsetY + 1, root.height - (root.chamferRight ? root.innerOffsetR : root.innerInsetY + 0.5)) }
            PathLine { x: Math.max(root.innerOffsetL + 1, root.width - root.innerOffsetR); y: root.height - root.innerInsetY - 0.5 }
            PathLine { x: root.innerOffsetL; y: root.height - root.innerInsetY - 0.5 }
            PathLine { x: root.innerInsetL + 0.5; y: Math.max(root.innerInsetY + 1, root.height - (root.chamferLeft ? root.innerOffsetL : root.innerInsetY + 0.5)) }
            PathLine { x: root.innerInsetL + 0.5; y: root.chamferLeft ? root.innerOffsetL : root.innerInsetY + 0.5 }
            PathLine { x: root.innerOffsetL; y: root.innerInsetY + 0.5 }
        }
    }

    Item {
        id: sheenClip
        x: root.sheenLeft
        y: root.innerInsetY + 1
        width: Math.max(0, root.width - root.sheenLeft - root.sheenRight)
        height: Math.max(0, root.height - (root.innerInsetY + 1) * 2)
        clip: true
        Accessible.ignored: true
        onWidthChanged: {
            if (!sheenAnim.running) sheenBar.x = -sheenBar.width
            if (!ignitionAnim.running) ignitionBar.x = -ignitionBar.width
        }

        Rectangle {
            id: sheenBar
            objectName: "playSheenBar"
            width: Math.max(24, Math.round(sheenClip.width * 0.42))
            height: sheenClip.height
            x: -width
            opacity: 0
            Accessible.ignored: true
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "transparent" }
                GradientStop { position: 0.5; color: Qt.alpha(Tokens.dark ? "#f3e3bd" : "#ffffff", 0.04) }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }

        SequentialAnimation {
            id: sheenAnim
            objectName: "playSheenAnimation"
            running: root.shouldAnimateSheen
            loops: Animation.Infinite
            onRunningChanged: {
                if (!running) {
                    sheenBar.x = -sheenBar.width
                    sheenBar.opacity = 0
                }
            }
            PropertyAction { target: sheenBar; property: "x"; value: -sheenBar.width }
            PropertyAction { target: sheenBar; property: "opacity"; value: 0 }
            ParallelAnimation {
                NumberAnimation {
                    target: sheenBar
                    property: "x"
                    from: -sheenBar.width
                    to: sheenClip.width
                    duration: Math.max(1, Tokens.panel * 5)
                    easing.type: Easing.InOutQuad
                }
                SequentialAnimation {
                    NumberAnimation {
                        target: sheenBar
                        property: "opacity"
                        from: 0
                        to: 1
                        duration: Math.max(1, Tokens.fast)
                        easing.type: Easing.OutQuad
                    }
                    PauseAnimation { duration: Math.max(0, Tokens.panel * 5 - Tokens.fast * 2) }
                    NumberAnimation {
                        target: sheenBar
                        property: "opacity"
                        from: 1
                        to: 0
                        duration: Math.max(1, Tokens.fast)
                        easing.type: Easing.InQuad
                    }
                }
            }
            PauseAnimation { duration: Math.max(0, Tokens.panel * 6) }
        }

        Rectangle {
            id: ignitionBar
            objectName: "playIgnitionBar"
            width: Math.max(28, Math.round(sheenClip.width * 0.55))
            height: sheenClip.height
            x: -width
            opacity: 0
            Accessible.ignored: true
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "transparent" }
                GradientStop { position: 0.5; color: Qt.alpha(Tokens.dark ? "#f3e3bd" : "#ffffff", 0.34) }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }

        SequentialAnimation {
            id: ignitionAnim
            objectName: "playIgnitionAnimation"
            loops: 1
            onRunningChanged: {
                if (!running) {
                    ignitionBar.x = -ignitionBar.width
                    ignitionBar.opacity = 0
                }
            }
            PropertyAction { target: ignitionBar; property: "x"; value: -ignitionBar.width }
            PropertyAction { target: ignitionBar; property: "opacity"; value: 0 }
            ParallelAnimation {
                NumberAnimation {
                    target: ignitionBar
                    property: "x"
                    from: -ignitionBar.width
                    to: sheenClip.width
                    duration: Math.max(1, Tokens.panel * 2)
                    easing.type: Easing.InOutCubic
                }
                SequentialAnimation {
                    NumberAnimation {
                        target: ignitionBar
                        property: "opacity"
                        from: 0
                        to: 1
                        duration: Math.max(1, Tokens.fast)
                        easing.type: Easing.OutQuad
                    }
                    PauseAnimation { duration: Math.max(0, Tokens.panel * 2 - Tokens.fast * 2) }
                    NumberAnimation {
                        target: ignitionBar
                        property: "opacity"
                        from: 1
                        to: 0
                        duration: Math.max(1, Tokens.fast)
                        easing.type: Easing.InQuad
                    }
                }
            }
        }
    }
}
