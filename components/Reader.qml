pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Ui
import "../Format.js" as Fmt

Item {
  id: root

  required property var app
  readonly property var mail: app.view
  readonly property color muted: Qt.darker(Color.foreground, 1.4)

  function scrollBy(dy) {
    body.contentY = Math.max(0, Math.min(body.contentHeight - body.height, body.contentY + dy))
  }

  // Theme colors go in as a stylesheet: the sanitizer strips every inline
  // style, so mail always follows the current Omarchy theme.
  // Newsletters wrap most of their text in links, so links get a quiet
  // accent tint instead of full accent + underline; the hover cursor and
  // the URL in the status bar show what is clickable. Links found in plain
  // text mail (class "u") keep an underline, since nothing else marks them.
  readonly property color linkColor: Qt.tint(Color.foreground, Util.alpha(Color.accent, 0.55))
  readonly property string css: "<style>"
    + "a { color: " + Fmt.cssColor(linkColor) + "; text-decoration: none; }"
    + " a.u { text-decoration: underline; }"
    + " p { margin-top: 0px; margin-bottom: 8px; }"
    + " .quote { color: " + Fmt.cssColor(muted) + "; }"
    + " blockquote { color: " + Fmt.cssColor(muted) + "; }</style>"

  Text {
    textFormat: Text.PlainText
    anchors.centerIn: parent
    visible: root.mail === null
    width: parent.width - Style.space(40)
    horizontalAlignment: Text.AlignHCenter
    wrapMode: Text.WordWrap
    text: root.app.loadingView ? "Loading…"
      : root.app.viewError.length > 0 ? root.app.viewError
      : "Select a message"
    color: root.app.viewError.length > 0 ? Color.urgent : root.muted
    font.family: Style.font.family
    font.pixelSize: Style.font.body
  }

  Column {
    id: header
    visible: root.mail !== null
    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.margins: Style.space(16)
    spacing: Style.space(6)

    Text {
      textFormat: Text.PlainText
      width: parent.width
      wrapMode: Text.WordWrap
      maximumLineCount: 3
      elide: Text.ElideRight
      text: root.mail ? (root.mail.subject || "(no subject)") : ""
      color: Color.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.heading
      font.bold: true
    }

    Item {
      width: parent.width
      height: fromText.implicitHeight

      Text {
        textFormat: Text.PlainText
        id: fromText
        anchors.left: parent.left
        anchors.right: dateText.left
        anchors.rightMargin: Style.space(10)
        elide: Text.ElideRight
        text: root.mail ? Fmt.fullPeople(root.mail.from) : ""
        color: Color.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
      }

      Text {
        textFormat: Text.PlainText
        id: dateText
        anchors.right: parent.right
        text: root.mail ? Fmt.longDate(root.mail.timestamp) : ""
        color: root.muted
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
      }
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      elide: Text.ElideRight
      text: root.mail ? "To: " + Fmt.people(root.mail.to)
        + (root.mail.cc.length > 0 ? "   Cc: " + Fmt.people(root.mail.cc) : "") : ""
      color: root.muted
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
    }

    Flow {
      width: parent.width
      spacing: Style.space(2)

      Button { iconText: Fmt.icons.reply; text: "Reply"; tooltipText: "r"; onClicked: root.app.compose("reply") }
      Button { iconText: Fmt.icons.replyAll; text: "All"; tooltipText: "Reply all (a)"; onClicked: root.app.compose("reply_all") }
      Button { iconText: Fmt.icons.forward; text: "Forward"; tooltipText: "f"; onClicked: root.app.compose("forward") }
      Item { width: Style.space(10); height: 1 }
      Button {
        visible: root.app.canArchive
        iconText: Fmt.icons.archive
        tooltipText: "Archive (e)"
        onClicked: root.app.moveCurrent("all")
      }
      Button {
        visible: root.app.canRestore
        iconText: Fmt.icons.inbox
        tooltipText: "Move to Inbox"
        onClicked: root.app.moveCurrent("inbox")
      }
      Button {
        visible: root.app.folderRole !== "trash"
        iconText: Fmt.icons.trash
        tooltipText: "Delete (#)"
        onClicked: root.app.moveCurrent("trash")
      }
      Button {
        iconText: Fmt.icons.unread
        tooltipText: "Mark unread (u)"
        onClicked: root.app.toggleSeen()
      }
      Button {
        iconText: root.app.currentFlagged ? Fmt.icons.flagged : Fmt.icons.starOff
        foreground: root.app.currentFlagged ? Color.accent : Color.foreground
        tooltipText: "Star (s)"
        onClicked: root.app.toggleStar()
      }
    }

    BorderSurface {
      visible: root.mail !== null && root.mail.remote_images > 0 && !root.mail.images_loaded
      width: parent.width
      implicitHeight: bannerRow.implicitHeight + Style.space(8)
      color: Util.alpha(Color.foreground, 0.04)
      radius: Style.cornerRadius
      borderSpec: Border.flat(Util.alpha(Color.foreground, 0.12), 1)

      Row {
        id: bannerRow
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left
        anchors.leftMargin: Style.space(10)
        spacing: Style.space(8)

        Text {
          textFormat: Text.PlainText
          anchors.verticalCenter: parent.verticalCenter
          text: Fmt.icons.image + "  " + (root.mail ? root.mail.remote_images : 0) + " remote images blocked"
          color: root.muted
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
        }
        Button {
          text: "Load images"
          tooltipText: "i"
          foreground: Color.accent
          fontSize: Style.font.caption
          onClicked: root.app.loadImages()
        }
      }
    }

    PanelSeparator { width: parent.width }
  }

  Flickable {
    id: body
    visible: root.mail !== null
    anchors.top: header.bottom
    anchors.topMargin: Style.space(4)
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: attachments.top
    anchors.leftMargin: Style.space(16)
    anchors.rightMargin: Style.space(8)
    clip: true
    contentWidth: width
    contentHeight: content.implicitHeight + Style.space(16)
    boundsBehavior: Flickable.StopAtBounds
    ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

    TextEdit {
      id: content
      width: body.width - Style.space(10)
      readOnly: true
      selectByMouse: true
      textFormat: TextEdit.RichText
      wrapMode: TextEdit.Wrap
      color: Color.foreground
      selectionColor: Style.selectionFill
      selectedTextColor: Color.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.body
      text: root.mail ? root.css + root.mail.html : ""
      onTextChanged: body.contentY = 0
      onLinkActivated: function(link) { root.app.openLink(link) }
      onLinkHovered: function(link) { root.app.hoveredLink = link }

      HoverHandler {
        cursorShape: content.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.IBeamCursor
      }
    }
  }

  Flow {
    id: attachments
    visible: root.mail !== null && root.mail.attachments.length > 0
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.margins: visible ? Style.space(12) : 0
    height: visible ? implicitHeight : 0
    spacing: Style.space(6)

    Repeater {
      model: root.mail ? root.mail.attachments : []

      BorderSurface {
        id: chip
        required property var modelData
        implicitWidth: chipRow.implicitWidth + Style.space(12)
        implicitHeight: chipRow.implicitHeight + Style.space(4)
        color: Util.alpha(Color.foreground, 0.04)
        radius: Style.cornerRadius
        borderSpec: Border.flat(Util.alpha(Color.foreground, 0.15), 1)

        Row {
          id: chipRow
          anchors.centerIn: parent
          spacing: Style.space(2)

          Text {
            textFormat: Text.PlainText
            anchors.verticalCenter: parent.verticalCenter
            text: Fmt.icons.attachment + " " + chip.modelData.name + "  " + Fmt.size(chip.modelData.size)
            color: Color.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            elide: Text.ElideMiddle
            width: Math.min(implicitWidth, Style.space(260))
          }
          Button {
            iconText: Fmt.icons.download
            tooltipText: "Save to Downloads"
            verticalPadding: Style.space(2)
            onClicked: root.app.saveAttachment(chip.modelData.index, false)
          }
          Button {
            iconText: Fmt.icons.open
            tooltipText: "Save and open"
            verticalPadding: Style.space(2)
            onClicked: root.app.saveAttachment(chip.modelData.index, true)
          }
        }
      }
    }
  }
}
