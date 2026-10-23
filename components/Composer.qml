pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import Quickshell.Io
import qs.Commons
import qs.Ui
import "../Format.js" as Fmt

Item {
  id: root

  required property var app
  // {to, cc, subject, body, in_reply_to, references, attachments, forward_folder, forward_uid, uidvalidity}
  required property var draft

  property var files: []
  property bool includeOriginals: true
  property bool showCopies: root.draft.cc.length > 0
  readonly property color muted: Qt.darker(Color.foreground, 1.4)
  readonly property bool dirty: toField.text !== root.draft.to || ccField.text !== root.draft.cc
    || bccField.text.length > 0 || subjectField.text !== root.draft.subject
    || bodyArea.text !== root.draft.body || files.length > 0

  function focusFirst() {
    if (toField.text.length === 0) toField.forceActiveFocus()
    else {
      bodyArea.forceActiveFocus()
      bodyArea.cursorPosition = 0
    }
  }

  function send() {
    var forwarding = root.draft.forward_uid > 0 && includeOriginals
    root.app.sendDraft({
      to: toField.text,
      cc: ccField.text,
      bcc: bccField.text,
      subject: subjectField.text,
      body: bodyArea.text,
      files: files,
      in_reply_to: root.draft.in_reply_to,
      references: root.draft.references,
      forward_folder: forwarding ? root.draft.forward_folder : "",
      forward_uid: forwarding ? root.draft.forward_uid : 0,
      uidvalidity: forwarding ? (root.draft.uidvalidity || 0) : 0
    })
  }

  function removeFile(path) {
    var next = []
    for (var i = 0; i < files.length; i++) if (files[i] !== path) next.push(files[i])
    files = next
  }

  function baseName(path) {
    var i = path.lastIndexOf("/")
    return i >= 0 ? path.substring(i + 1) : path
  }

  Component.onCompleted: Qt.callLater(focusFirst)

  // Shortcuts rather than Keys: the text fields swallow Enter themselves.
  Shortcut {
    sequences: ["Ctrl+Return", "Ctrl+Enter"]
    enabled: root.visible && !root.app.confirming
    onActivated: root.send()
  }
  Shortcut {
    sequence: "Escape"
    enabled: root.visible && !root.app.confirming
    onActivated: root.app.discardCompose()
  }

  // zenity is a GTK file chooser; it prints one path per line.
  Process {
    id: picker
    command: ["zenity", "--file-selection", "--multiple", "--separator=\n", "--title=Attach files"]
    stdout: StdioCollector {
      onStreamFinished: {
        var lines = String(text || "").split("\n")
        var next = root.files.slice()
        for (var i = 0; i < lines.length; i++) {
          var p = lines[i].trim()
          if (p.length > 0 && next.indexOf(p) === -1) next.push(p)
        }
        root.files = next
      }
    }
  }

  Column {
    id: fields
    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.margins: Style.space(16)
    spacing: Style.space(6)

    Text {
      textFormat: Text.PlainText
      text: root.draft.forward_uid > 0 ? "Forward" : (root.draft.in_reply_to.length > 0 ? "Reply" : "New message")
      color: Color.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.heading
      font.bold: true
    }

    Row {
      width: parent.width
      spacing: Style.space(6)
      TextField {
        id: toField
        width: parent.width - copiesButton.width - parent.spacing
        placeholderText: "To"
        text: root.draft.to
        KeyNavigation.tab: root.showCopies ? ccField : subjectField
      }
      Button {
        id: copiesButton
        text: "Cc/Bcc"
        selected: root.showCopies
        onClicked: root.showCopies = !root.showCopies
      }
    }

    TextField {
      id: ccField
      visible: root.showCopies
      width: parent.width
      placeholderText: "Cc"
      text: root.draft.cc
      KeyNavigation.tab: bccField
    }

    TextField {
      id: bccField
      visible: root.showCopies
      width: parent.width
      placeholderText: "Bcc"
      KeyNavigation.tab: subjectField
    }

    TextField {
      id: subjectField
      width: parent.width
      placeholderText: "Subject"
      text: root.draft.subject
      KeyNavigation.tab: bodyArea
    }
  }

  ScrollView {
    id: bodyScroll
    anchors.top: fields.bottom
    anchors.topMargin: Style.space(8)
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: footer.top
    anchors.leftMargin: Style.space(16)
    anchors.rightMargin: Style.space(16)
    anchors.bottomMargin: Style.space(8)
    ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

    TextArea {
      id: bodyArea
      text: root.draft.body
      wrapMode: TextArea.Wrap
      textFormat: TextEdit.PlainText
      color: Color.foreground
      selectionColor: Style.selectionFill
      selectedTextColor: Color.foreground
      placeholderText: "Write your message…"
      placeholderTextColor: root.muted
      font.family: Style.font.family
      font.pixelSize: Style.font.body
      padding: Style.space(10)
      background: BorderSurface {
        color: Style.controlFill(bodyArea.activeFocus, bodyArea.hovered, Color.foreground, Color.accent)
        borderSpec: Border.controlSpec(bodyArea.activeFocus ? "focus" : (bodyArea.hovered ? "hover-cursor" : "normal"), Color.foreground, Color.accent)
        radius: Style.cornerRadius
      }
    }
  }

  Column {
    id: footer
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.margins: Style.space(16)
    spacing: Style.space(8)

    Flow {
      width: parent.width
      spacing: Style.space(6)
      visible: root.files.length > 0 || (root.draft.forward_uid > 0 && root.draft.attachments.length > 0)

      Button {
        visible: root.draft.forward_uid > 0 && root.draft.attachments.length > 0
        iconText: Fmt.icons.attachment
        text: root.draft.attachments.length + " original attachment" + (root.draft.attachments.length > 1 ? "s" : "")
        selected: root.includeOriginals
        tooltipText: root.includeOriginals ? "Included. Click to leave out" : "Left out. Click to include"
        onClicked: root.includeOriginals = !root.includeOriginals
      }

      Repeater {
        model: root.files
        Button {
          required property string modelData
          iconText: Fmt.icons.close
          text: root.baseName(modelData)
          tooltipText: modelData
          onClicked: root.removeFile(modelData)
        }
      }
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: root.app.sendError.length > 0
      wrapMode: Text.WordWrap
      text: root.app.sendError
      color: Color.urgent
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
    }

    Row {
      spacing: Style.space(8)

      Button {
        bordered: true
        iconText: Fmt.icons.sent
        text: root.app.sending ? "Sending…" : "Send"
        foreground: Color.accent
        tooltipText: "Ctrl+Enter"
        onClicked: root.send()
      }
      Button {
        iconText: Fmt.icons.attachment
        text: "Attach"
        onClicked: if (!picker.running) picker.running = true
      }
      Button {
        iconText: Fmt.icons.trash
        text: "Discard"
        tooltipText: "Esc"
        onClicked: root.app.discardCompose()
      }
    }
  }
}
