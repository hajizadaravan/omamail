import QtQuick
import qs.Commons

// A clickable full-width row painted with the shell's shared state tokens,
// so hover and selection look exactly like Omarchy's own panels.
Rectangle {
  id: root

  property bool selected: false
  property bool hasCursor: false
  readonly property bool hot: mouse.containsMouse || hasCursor

  signal clicked()
  signal doubleClicked()

  radius: Style.cornerRadius
  color: mouse.pressed ? Style.pressedFill
    : selected ? Style.selectedFill
    : hot ? Style.hoverFill
    : "transparent"
  Behavior on color { ColorAnimation { duration: 90 } }

  MouseArea {
    id: mouse
    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: root.clicked()
    onDoubleClicked: root.doubleClicked()
  }
}
