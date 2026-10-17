import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Ui
import "../Format.js" as Fmt

Item {
  id: root

  required property var app
  property alias searchField: search
  property alias view: list

  readonly property color muted: Qt.darker(Color.foreground, 1.4)

  TextField {
    id: search
    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.margins: Style.space(10)
    placeholderText: Fmt.icons.search + "  Search  (/)"
    onAccepted: root.app.search(text)
    Keys.onEscapePressed: {
      if (text.length > 0) {
        text = ""
        root.app.search("")
      }
      root.app.focusList()
    }
    Keys.onDownPressed: root.app.focusList()
  }

  Text {
    textFormat: Text.PlainText
    id: heading
    anchors.top: search.bottom
    anchors.topMargin: Style.space(8)
    anchors.left: parent.left
    anchors.leftMargin: Style.space(12)
    anchors.right: parent.right
    anchors.rightMargin: Style.space(12)
    elide: Text.ElideRight
    color: root.muted
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: root.app.query.length > 0
      ? "Results for “" + root.app.query + "” · " + root.app.total
      : root.app.folderName + " · " + root.app.total
  }

  ListView {
    id: list
    anchors.top: heading.bottom
    anchors.topMargin: Style.space(6)
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.leftMargin: Style.space(6)
    anchors.rightMargin: Style.space(6)
    clip: true
    spacing: 1
    model: root.app.messages
    currentIndex: root.app.cursor
    highlightFollowsCurrentItem: false
    boundsBehavior: Flickable.StopAtBounds
    ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

    onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)
    onAtYEndChanged: if (atYEnd && count > 0) root.app.loadMore()

    delegate: RowSurface {
      id: row
      required property int index
      required property int uid
      required property string fromName
      required property string subject
      required property int timestamp
      required property bool seen
      required property bool flagged
      required property bool attachment

      width: ListView.view.width
      height: Style.space(52)
      selected: index === root.app.cursor
      onClicked: root.app.select(index)

      Rectangle {
        id: dot
        anchors.left: parent.left
        anchors.leftMargin: Style.space(7)
        anchors.top: parent.top
        anchors.topMargin: Style.space(14)
        width: Style.space(6)
        height: width
        radius: width / 2
        color: Color.accent
        visible: !row.seen
      }

      Text {
        textFormat: Text.PlainText
        id: date
        anchors.right: parent.right
        anchors.rightMargin: Style.space(10)
        anchors.top: parent.top
        anchors.topMargin: Style.space(8)
        text: Fmt.shortDate(row.timestamp)
        color: row.seen ? root.muted : Color.accent
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
      }

      Text {
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.leftMargin: Style.space(20)
        anchors.right: date.left
        anchors.rightMargin: Style.space(8)
        anchors.top: parent.top
        anchors.topMargin: Style.space(7)
        text: row.fromName.length > 0 ? row.fromName : "(no sender)"
        elide: Text.ElideRight
        color: Color.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        font.bold: !row.seen
      }

      Text {
        textFormat: Text.PlainText
        id: marks
        anchors.right: parent.right
        anchors.rightMargin: Style.space(10)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Style.space(8)
        text: (row.attachment ? Fmt.icons.attachment : "") + (row.flagged ? " " + Fmt.icons.flagged : "")
        color: row.flagged ? Color.accent : root.muted
        font.family: Style.font.family
        font.pixelSize: Style.font.bodySmall
      }

      Text {
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.leftMargin: Style.space(20)
        anchors.right: marks.left
        anchors.rightMargin: Style.space(8)
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Style.space(8)
        text: row.subject.length > 0 ? row.subject : "(no subject)"
        elide: Text.ElideRight
        color: row.seen ? root.muted : Color.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.bodySmall
      }
    }

    footer: Item {
      width: ListView.view ? ListView.view.width : 0
      height: root.app.loadingList ? Style.space(36) : 0
      Text {
        textFormat: Text.PlainText
        anchors.centerIn: parent
        visible: root.app.loadingList
        text: "Loading…"
        color: root.muted
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
      }
    }
  }

  Text {
    textFormat: Text.PlainText
    anchors.centerIn: list
    visible: !root.app.loadingList && list.count === 0
    text: root.app.listError.length > 0 ? root.app.listError : (root.app.query.length > 0 ? "No matches" : "No messages")
    color: root.app.listError.length > 0 ? Color.urgent : root.muted
    width: list.width - Style.space(24)
    horizontalAlignment: Text.AlignHCenter
    wrapMode: Text.WordWrap
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
  }
}
