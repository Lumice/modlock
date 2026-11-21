import QtQuick
import QtQuick.Shapes
import Deadlock.Native

Rectangle {
    id: root
    color: Theme.surfaceLow
    radius: Theme.radiusCard
    border.color: Theme.ruleStrong
    clip: true
    Accessible.ignored: true
    Shape {
        anchors.fill: parent
        ShapePath {
            strokeWidth: 0
            fillGradient: RadialGradient {
                centerX: root.width / 2; centerY: root.height
                centerRadius: root.width * 0.7
                focalX: centerX; focalY: centerY
                GradientStop { position: 0; color: Theme.surfacePressed }
                GradientStop { position: 1; color: Theme.surfaceLow }
            }
            startX: root.radius; startY: 1
            PathLine { x: root.width - root.radius; y: 1 }
            PathQuad { x: root.width - 1; y: root.radius; controlX: root.width - 1; controlY: 1 }
            PathLine { x: root.width - 1; y: root.height - root.radius }
            PathQuad { x: root.width - root.radius; y: root.height - 1; controlX: root.width - 1; controlY: root.height - 1 }
            PathLine { x: root.radius; y: root.height - 1 }
            PathQuad { x: 1; y: root.height - root.radius; controlX: 1; controlY: root.height - 1 }
            PathLine { x: 1; y: root.radius }
            PathQuad { x: root.radius; y: 1; controlX: 1; controlY: 1 }
        }
    }
    Repeater {
        model: 35
        Shape {
            id: ray
            required property int index
            anchors.fill: parent
            ShapePath {
                strokeColor: Qt.alpha(Theme.brass, 0.09)
                strokeWidth: 1
                fillColor: "transparent"
                startX: root.width / 2; startY: root.height
                PathLine {
                    x: root.width / 2 + Math.cos(Math.PI * (ray.index + 1) / 36) * root.width
                    y: root.height - Math.sin(Math.PI * (ray.index + 1) / 36) * root.width
                }
            }
        }
    }
    Repeater {
        model: 4
        Item {
            required property int index
            x: index % 2 ? root.width - 26 : 8
            y: index < 2 ? 8 : root.height - 26
            width: 18; height: 18
            Rectangle { width: 18; height: 1; y: parent.index < 2 ? 0 : 17; color: Qt.alpha(Theme.brass, 0.55) }
            Rectangle { width: 1; height: 18; x: parent.index % 2 ? 17 : 0; color: Qt.alpha(Theme.brass, 0.55) }
        }
    }
}
