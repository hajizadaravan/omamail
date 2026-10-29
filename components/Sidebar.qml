pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Ui
import "../Format.js" as Fmt

Item {
  id: root

  required property var app

  Column {
    id: header
    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.margins: Style.space(12)
    spacing: Style.space(2)

    Text {
      textFormat: Text.PlainText
      text: "Omamail"
      color: Color.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.heading
      font.bold: true
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      text: root.app.email
      elide: Text.ElideMiddle
      color: Qt.darker(Color.foreground, 1.4)
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
    }

    Item { width: 1; height: Style.space(10) }

    Button {
      width: parent.width
      bordered: true
      leftAlign: true
      iconText: Fmt.icons.compose
      text: "Compose"
      tooltipText: "c"
      onClicked: root.app.compose("new")
    }
  }

  ListView {
    id: list
    anchors.top: header.bottom
    anchors.topMargin: Style.space(12)
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: footer.top
    anchors.leftMargin: Style.space(6)
    anchors.rightMargin: Style.space(6)
    clip: true
    spacing: 1
    model: root.app.folders
    boundsBehavior: Flickable.StopAtBounds
    ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

    delegate: RowSurface {
      id: row
      required property var modelData
      required property int index
      width: ListView.view.width
      height: Style.space(28)
      selected: modelData.path === root.app.folder
      onClicked: root.app.selectFolder(modelData.path)

      Text {
        textFormat: Text.PlainText
        id: glyph
        anchors.left: parent.left
        anchors.leftMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(18)
        text: Fmt.folderIcon(row.modelData.role)
        color: row.selected ? Color.accent : Qt.darker(Color.foreground, 1.2)
        font.family: Style.font.family
        font.pixelSize: Style.font.icon
      }

      Text {
        textFormat: Text.PlainText
        anchors.left: glyph.right
        anchors.leftMargin: Style.space(6)
        anchors.right: count.left
        anchors.rightMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        text: row.modelData.name
        elide: Text.ElideRight
        color: Color.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        font.bold: row.selected
      }

      Text {
        textFormat: Text.PlainText
        id: count
        anchors.right: parent.right
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        visible: row.modelData.unread > 0
        text: row.modelData.unread
        color: Color.accent
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
        font.bold: true
      }
    }
  }

  Row {
    id: footer
    anchors.left: parent.left
    anchors.bottom: parent.bottom
    anchors.margins: Style.space(8)
    spacing: Style.space(4)

    Button {
      iconText: Fmt.icons.refresh
      tooltipText: "Refresh (R)"
      onClicked: root.app.refreshAll()
    }

    Button {
      iconText: Fmt.icons.logout
      tooltipText: "Sign out"
      onClicked: root.app.confirmSignOut()
    }

    Button {
      iconText: Fmt.icons.about
      tooltipText: "About (?)"
      onClicked: root.app.showAbout()
    }
  }
}
