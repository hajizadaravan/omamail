import QtQuick
import qs.Commons
import qs.Ui

Item {
  id: root

  required property var app

  function focusFirst() {
    if (emailField.text.length === 0) emailField.forceActiveFocus()
    else passwordField.forceActiveFocus()
  }

  function submit() {
    if (root.app.busy) return
    root.app.signIn(emailField.text, passwordField.text)
  }

  Component.onCompleted: {
    emailField.text = root.app.email
    Qt.callLater(focusFirst)
  }

  Column {
    anchors.centerIn: parent
    width: Math.min(parent.width - Style.space(40), Style.space(420))
    spacing: Style.space(14)

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
      width: parent.width
      wrapMode: Text.WordWrap
      color: Qt.darker(Color.foreground, 1.4)
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: "Sign in with your Gmail address and an app password. Google requires 2-Step Verification for app passwords. The password is stored in your system keyring, never in a file."
    }

    TextField {
      id: emailField
      width: parent.width
      placeholderText: "you@gmail.com"
      inputMethodHints: Qt.ImhEmailCharactersOnly | Qt.ImhNoAutoUppercase
      onAccepted: passwordField.forceActiveFocus()
    }

    TextField {
      id: passwordField
      width: parent.width
      password: true
      placeholderText: "16-character app password"
      onAccepted: root.submit()
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: root.app.signInError.length > 0
      wrapMode: Text.WordWrap
      color: Color.urgent
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: root.app.signInError
    }

    Row {
      spacing: Style.space(10)

      Button {
        bordered: true
        focusable: true
        text: root.app.busy ? "Signing in…" : "Sign in"
        onClicked: root.submit()
      }

      Button {
        text: "Create an app password " + "\u{f03cc}"
        foreground: Color.accent
        tooltipText: "myaccount.google.com/apppasswords"
        onClicked: Qt.openUrlExternally("https://myaccount.google.com/apppasswords")
      }
    }
  }
}
