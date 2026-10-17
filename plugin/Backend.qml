import QtQuick
import Quickshell.Io

// Bridge to `bin/omamail serve`: one JSON object per line each way. The
// process lives exactly as long as the window — the panel Loader unloads
// this item on close, which ends the process and its IMAP connections.
Item {
  id: root

  property string binary: ""
  property bool started: false
  property int nextId: 1
  property var callbacks: ({})
  property var queue: []

  signal changed(string folder)
  signal failed(string message)

  function request(cmd, args, callback) {
    var msg = {}
    for (var k in (args || {})) msg[k] = args[k]
    msg.id = nextId++
    msg.cmd = cmd
    if (callback) callbacks[msg.id] = callback
    var line = JSON.stringify(msg) + "\n"
    if (started) proc.write(line)
    else queue.push(line)
    return msg.id
  }

  function handleLine(line) {
    var msg
    try {
      msg = JSON.parse(line)
    } catch (e) {
      console.warn("omamail: unreadable backend line")
      return
    }
    if (msg.event === "changed") {
      root.changed(msg.folder || "INBOX")
      return
    }
    var cb = callbacks[msg.id]
    if (!cb) return
    delete callbacks[msg.id]
    cb(msg.ok === true, msg.ok ? msg.data : (msg.error || "Unknown error"), msg.code || "")
  }

  function failAll(message) {
    var pending = callbacks
    callbacks = ({})
    for (var id in pending) {
      try { pending[id](false, message, "error") } catch (e) {}
    }
  }

  Process {
    id: proc
    command: [root.binary, "serve"]
    running: root.binary !== ""
    stdinEnabled: true

    stdout: SplitParser {
      onRead: function(line) { root.handleLine(line) }
    }
    stderr: SplitParser {
      onRead: function(line) { console.warn("omamail:", line) }
    }

    onStarted: {
      root.started = true
      var pending = root.queue
      root.queue = []
      for (var i = 0; i < pending.length; i++) proc.write(pending[i])
    }

    onExited: function(code, status) {
      root.started = false
      root.failAll("The mail backend stopped (exit " + code + ")")
      root.failed("The mail backend stopped unexpectedly. Restarting…")
      restartTimer.restart()
    }
  }

  Timer {
    id: restartTimer
    interval: 1500
    onTriggered: proc.running = true
  }
}
