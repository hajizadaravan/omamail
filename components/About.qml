pragma ComponentBehavior: Bound

import QtQuick
import qs.Commons
import qs.Ui
import "../Format.js" as Fmt

// Who made Omamail, where it lives, and which build is running.
// Built like the shell's ConfirmDialog: a card over a scrim.
Item {
  id: root

  required property var app
  property bool opened: false

  readonly property string repo: "https://github.com/hajizadaravan/omamail"
  readonly property var rows: [
    { label: "Version", value: root.app.version.length > 0 ? root.app.version : "unknown" },
    { label: "Author", value: "Ravan Hajizada" },
    { label: "Source", value: "github.com/hajizadaravan/omamail", link: root.repo },
    { label: "Issues", value: "Report a bug or ask for a feature", link: root.repo + "/issues" },
    { label: "Plugin", value: root.app.pluginId },
    { label: "License", value: "MIT" },
    { label: "Account", value: root.app.email }
  ]

  signal closed()

  visible: opened

  Rectangle {
    anchors.fill: parent
    color: Util.alpha(Color.background, 0.7)

    MouseArea { anchors.fill: parent; onClicked: root.closed() }

    BorderSurface {
      id: card
      width: Math.min(parent.width - Style.space(32), Style.space(440))
      height: card.contentTopInset + card.contentBottomInset + content.implicitHeight
      anchors.centerIn: parent
      color: Color.background
      borderSpec: Border.flat(Color.accent, Style.normalBorderWidth)
      padding: Style.space(20)
      radius: Style.cornerRadius

      MouseArea { anchors.fill: parent; onClicked: {} }

      Column {
        id: content
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: card.contentTopInset
        anchors.leftMargin: card.contentLeftInset
        anchors.rightMargin: card.contentRightInset
        spacing: Style.space(14)

        Row {
          spacing: Style.space(14)

          Image {
            width: Style.space(48)
            height: width
            source: Qt.resolvedUrl("../share/omamail.svg")
            sourceSize: Qt.size(width * 2, height * 2)
            smooth: false
          }

          Column {
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(2)

            Text {
              textFormat: Text.PlainText
              text: "Omamail"
              color: Color.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.display
              font.bold: true
            }

            Text {
              textFormat: Text.PlainText
              text: "A light, keyboard-driven Gmail client for the Omarchy shell."
              color: Qt.darker(Color.foreground, 1.4)
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
            }
          }
        }

        Rectangle {
          width: parent.width
          height: 1
          color: Util.alpha(Color.foreground, 0.12)
        }

        Column {
          width: parent.width
          spacing: Style.space(6)

          Repeater {
            model: root.rows

            Row {
              id: row
              required property var modelData
              width: parent.width
              spacing: Style.space(12)
              visible: modelData.value.length > 0

              Text {
                textFormat: Text.PlainText
                width: Style.space(70)
                text: row.modelData.label
                color: Qt.darker(Color.foreground, 1.6)
                font.family: Style.font.family
                font.pixelSize: Style.font.bodySmall
              }

              Text {
                textFormat: Text.PlainText
                width: parent.width - Style.space(82)
                text: row.modelData.value
                elide: Text.ElideMiddle
                color: row.modelData.link ? Color.accent : Color.foreground
                font.family: Style.font.family
                font.pixelSize: Style.font.bodySmall
                font.underline: link.containsMouse

                MouseArea {
                  id: link
                  anchors.fill: parent
                  enabled: !!row.modelData.link
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onClicked: Qt.openUrlExternally(row.modelData.link)
                  onContainsMouseChanged: root.app.hoveredLink = containsMouse ? row.modelData.link : ""
                }
              }
            }
          }
        }

        Item {
          width: parent.width
          height: closeButton.height

          Text {
            textFormat: Text.PlainText
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            text: "Esc to close"
            color: Qt.darker(Color.foreground, 1.6)
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
          }

          Button {
            id: closeButton
            anchors.right: parent.right
            bordered: true
            iconText: Fmt.icons.close
            text: "Close"
            onClicked: root.closed()
          }
        }
      }
    }
  }
}
