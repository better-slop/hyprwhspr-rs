import Quickshell
import QtQuick
import Quickshell.Io

PanelWindow {
    id: statusIndicator
    implicitWidth: 48
    implicitHeight: implicitWidth

    // focus should never be given to this panel
    mask: Region {}

    FileView {
        path: `${Quickshell.env("HOME")}/.cache/hyprwhspr-rs/status.json`
        watchChanges: true
        onFileChanged: reload()
        JsonAdapter {
            property string alt: ""
            onAltChanged: statusIndicator.visible = alt === "active"
        }
    }

    color: "transparent"
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop {
                position: 0
                color: "#74c7ec"
            }
            GradientStop {
                position: 1
                color: "#89b4fa"
            }
        }
        radius: parent.height
    }
    Rectangle {
        anchors {
            fill: parent
            margins: 2
        }
        radius: parent.height
        color: "#1e1e2e"
    }
    Image {
        source: "./mic.svg"
        anchors.centerIn: parent
    }
}
