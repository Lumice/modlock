import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native

Pane {
    id: root
    required property FrameMetrics metrics
    padding: 16
    ColumnLayout {
        anchors.fill: parent
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            spacing: 24
            Label { text: qsTr("Display: %1 Hz").arg(root.metrics.refreshRate.toFixed(1)) }
            Label {
                text: root.metrics.samples > 1
                    ? qsTr("Updates: %1/s").arg(root.metrics.fps.toFixed(1)) : qsTr("Updates: idle")
            }
            Label { text: qsTr("Frame interval p95: %1 ms").arg(root.metrics.p95.toFixed(2)) }
            Item { Layout.fillWidth: true }
            CheckBox {
                id: animate
                objectName: "animationTest"
                text: qsTr("Animate test strip")
            }
        }
        Rectangle {
            id: track
            Layout.fillWidth: true
            Layout.preferredHeight: 8
            color: Theme.alternate
            visible: animate.checked
            Rectangle {
                id: marker
                width: 40
                height: parent.height
                color: Theme.accent
                XAnimator {
                    id: movement
                    target: marker
                    from: 0
                    to: Math.max(0, track.width - marker.width)
                    duration: 2400
                    loops: Animation.Infinite
                    running: animate.checked && root.visible && !Preferences.reduceMotion && root.metrics.collecting && movement.to > 0
                }
            }
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Idle screens render only when needed. These are Qt frame-swap timings, not measured display presentation.")
            color: Theme.secondary
            font.pointSize: Theme.small
            wrapMode: Text.WordWrap
        }
    }
}

