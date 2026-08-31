import QtQuick
import QtQuick.Window

Window {
    id: root

    width: 1280
    height: 720
    visible: true
    title: "Marina"
    color: "#1f2430"

    opacity: 0.4

    Text {
        anchors.centerIn: parent
        color: "#e5e9f0"
        text: "Marina"
        font.pixelSize: 36
        opacity: 0.3
    }
}
