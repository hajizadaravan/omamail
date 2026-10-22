pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Format.js" as Fmt
import "components"

// Omamail: a three-pane Gmail window hosted by omarchy-shell as a panel
// plugin. The shell loads this file on summon and unloads it on hide, so
// nothing (window, backend process, IMAP connections) lingers while closed.
Item {
  id: mailApp

  readonly property string pluginId: "reavann.omamail"

  // ---- host lifecycle ------------------------------------------------------
  property var shell: null
  property bool opened: false
  property bool closingFromHost: false

  function open(payloadJson) {
    closingFromHost = false
    opened = true
    window.visible = true
    var target = null
    try {
      var p = payloadJson ? JSON.parse(String(payloadJson)) : null
      if (p && typeof p.folder === "string") target = p
    } catch (e) {}
    if (target) {
      if (phase === "mail") showMessage(target.folder, target.uid, true)
      else pendingTarget = target
    }
    Qt.callLater(focusList)
  }

  function close() {
    closingFromHost = true
    opened = false
    window.visible = false
    closingFromHost = false
  }

  function requestClose() {
    if (shell && typeof shell.hide === "function") shell.hide(pluginId)
    else close()
  }

  // ---- state ---------------------------------------------------------------
  // Built by install.sh outside the plugin's git checkout (see there).
  readonly property string backendPath: Quickshell.env("HOME") + "/.local/lib/omamail/omamail"
  readonly property string installScript: decodeURIComponent(String(Qt.resolvedUrl("install.sh")).replace(/^file:\/\//, ""))
  property bool backendReady: false

  // `omarchy plugin add` only clones; the backend is built by install.sh.
  // Without it, say so instead of restarting a missing binary forever.
  Process {
    command: ["test", "-x", mailApp.backendPath]
    running: true
    onExited: function(code) {
      if (code === 0) {
        mailApp.backendReady = true
        mailApp.start()
      } else {
        mailApp.phase = "setup"
      }
    }
  }

  property Backend backend: Backend {
    binary: mailApp.backendReady ? mailApp.backendPath : ""
    onChanged: function(folder) { mailApp.onServerChange(folder) }
    onFailed: function(message) { mailApp.flash(message) }
  }

  property string phase: "loading"   // loading | signin | mail
  property string email: ""
  property string signInError: ""
  property bool busy: false
  property var pendingTarget: null

  property var folders: []
  property string folder: "INBOX"
  readonly property var currentFolder: folderByPath(folder)
  readonly property string folderRole: roleOf(folder)
  readonly property string folderName: currentFolder ? currentFolder.name : folder
  readonly property bool canArchive: folderRole === "inbox" || folderRole === ""
  readonly property bool canRestore: folderRole === "trash" || folderRole === "junk"

  property ListModel messages: ListModel {}
  property int total: 0
  property bool loadingList: false
  property string listError: ""
  property string query: ""
  // The selection is tracked by UID; `cursor` is only its current row.
  property int selectedUid: -1
  property int cursor: -1
  property int listToken: 0
  property bool refreshPending: false

  property var view: null
  property string viewFolder: ""
  property bool loadingView: false
  property string viewError: ""
  property int viewToken: 0
  property bool currentFlagged: false

  property var draft: null
  property bool sending: false
  property string sendError: ""
  property bool confirming: false
  property var confirmAction: null

  property string status: ""
  property string hoveredLink: ""

  function flash(text) {
    status = text
    statusTimer.restart()
  }

  function handleError(message, code) {
    if (code === "auth" || code === "not_configured") {
      signInError = code === "auth" ? message : ""
      phase = "signin"
      return
    }
    flash(message)
  }

  // ---- account -------------------------------------------------------------
  function start() {
    backend.request("status", {}, function(ok, data) {
      if (!ok) { phase = "signin"; return }
      email = data.email || ""
      if (data.configured) enterMail()
      else phase = "signin"
    })
  }

  function signIn(address, password) {
    busy = true
    signInError = ""
    backend.request("login", { email: address, password: password }, function(ok, data) {
      busy = false
      if (!ok) { signInError = data; return }
      email = data.email
      enterMail()
    })
  }

  function confirmSignOut() {
    askConfirm("Sign out of " + email + "? The app password is removed from the keyring.", "Sign out", function() {
      backend.request("logout", {}, function() {
        folders = []
        messages.clear()
        view = null
        draft = null
        phase = "signin"
      })
    })
  }

  // The backend answers in order: the list (or the clicked message) first,
  // then the folder counts, which take a round trip per label.
  function enterMail() {
    phase = "mail"
    signInError = ""
    var target = pendingTarget
    pendingTarget = null
    if (target) showMessage(target.folder, target.uid, true)
    else selectFolder(folder)
    loadFolders()
  }

  // ---- folders -------------------------------------------------------------
  function folderByPath(path) {
    for (var i = 0; i < folders.length; i++) if (folders[i].path === path) return folders[i]
    return null
  }

  function roleOf(path) {
    var f = folderByPath(path)
    return f && f.role ? f.role : ""
  }

  function loadFolders() {
    backend.request("folders", {}, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      folders = data
    })
  }

  function setUnread(path, compute) {
    var next = []
    for (var i = 0; i < folders.length; i++) {
      var f = folders[i]
      if (f.path === path) f = { path: f.path, name: f.name, role: f.role, unread: Math.max(0, compute(f.unread)) }
      next.push(f)
    }
    folders = next
  }

  function adjustUnread(path, delta) {
    setUnread(path, function(n) { return n + delta })
  }

  function selectFolder(path) {
    if (draft) return
    folder = path
    setQuery("")
    view = null
    viewError = ""
    loadList(true)
  }

  function selectFolderBy(step) {
    if (folders.length === 0) return
    var i = 0
    for (; i < folders.length; i++) if (folders[i].path === folder) break
    var next = (i + step + folders.length) % folders.length
    selectFolder(folders[next].path)
  }

  // ---- message list --------------------------------------------------------
  function flatten(m) {
    return {
      uid: m.uid,
      fromName: Fmt.person(m.from[0]),
      fromEmail: m.from.length > 0 ? m.from[0].email : "",
      subject: m.subject,
      timestamp: m.timestamp,
      seen: m.seen,
      flagged: m.flagged,
      attachment: m.attachment
    }
  }

  function sameRow(a, b) {
    return a.uid === b.uid && a.seen === b.seen && a.flagged === b.flagged && a.subject === b.subject
      && a.fromName === b.fromName && a.timestamp === b.timestamp && a.attachment === b.attachment
  }

  // Updates the model in place, so a refresh keeps the scroll position and
  // only repaints rows that changed.
  function applyRows(rows) {
    for (var i = 0; i < rows.length; i++) {
      if (i < messages.count) {
        if (!sameRow(messages.get(i), rows[i])) messages.set(i, rows[i])
      } else {
        messages.append(rows[i])
      }
    }
    if (messages.count > rows.length) messages.remove(rows.length, messages.count - rows.length)
  }

  function syncCursor() {
    cursor = selectedUid >= 0 ? indexOfUid(selectedUid) : -1
  }

  function setQuery(text) {
    query = text
    if (listView() && listView().searchField.text !== text) listView().searchField.text = text
  }

  // reset: a different folder or query, start empty. Otherwise refresh the
  // newest page and keep older rows that were already loaded below it.
  // quiet: a background refresh, whose failure is not worth a message.
  function loadList(reset, quiet) {
    if (reset) {
      messages.clear()
      total = 0
      selectedUid = -1
      cursor = -1
    }
    // A background refresh keeps a shown error until it succeeds.
    if (!quiet) listError = ""
    loadingList = true
    var token = ++listToken
    var searching = query.length > 0
    backend.request(searching ? "search" : "list", { folder: folder, query: query }, function(ok, data, code) {
      if (token !== listToken) return
      loadingList = false
      runPendingRefresh()
      if (!ok) {
        if (quiet) return
        listError = data
        return handleError(data, code)
      }
      listError = ""
      var rows = []
      for (var i = 0; i < data.messages.length; i++) rows.push(flatten(data.messages[i]))
      // Keep older loaded rows only if the new page reaches down to them;
      // otherwise there would be a silent gap between the two.
      var overlaps = rows.length > 0 && messages.count > 0 && rows[rows.length - 1].uid <= messages.get(0).uid
      if (!searching && !reset && overlaps) {
        var oldest = rows[rows.length - 1].uid
        for (var j = 0; j < messages.count; j++) {
          var row = messages.get(j)
          if (row.uid < oldest) rows.push(flatten({ uid: row.uid, from: [{ name: row.fromName, email: row.fromEmail }],
            subject: row.subject, timestamp: row.timestamp, seen: row.seen, flagged: row.flagged, attachment: row.attachment }))
        }
      }
      applyRows(rows)
      total = data.total
      syncCursor()
    })
  }

  function loadMore() {
    if (loadingList || query.length > 0 || messages.count === 0 || messages.count >= total) return
    loadingList = true
    var token = ++listToken
    var before = messages.get(messages.count - 1).uid
    backend.request("list", { folder: folder, before_uid: before }, function(ok, data, code) {
      if (token !== listToken) return
      loadingList = false
      runPendingRefresh()
      if (!ok) return handleError(data, code)
      for (var i = 0; i < data.messages.length; i++) {
        var m = data.messages[i]
        if (indexOfUid(m.uid) < 0) messages.append(flatten(m))
      }
      total = data.total
    })
  }

  function search(text) {
    setQuery(text.trim())
    view = null
    loadList(true)
    if (query.length > 0) focusList()
  }

  // The IDLE connection reports any change, including our own flag edits:
  // refresh what is cheap (one count, one page), not every folder.
  function onServerChange(changedFolder) {
    if (phase !== "mail") return
    backend.request("unread", { folder: changedFolder }, function(ok, data) {
      if (ok) setUnread(changedFolder, function() { return data })
    })
    if (folder !== changedFolder || query.length > 0) return
    if (loadingList) refreshPending = true
    else loadList(false)
  }

  // A change that arrived while a page was loading is applied right after.
  function runPendingRefresh() {
    if (!refreshPending) return
    refreshPending = false
    Qt.callLater(function() { if (!mailApp.loadingList && mailApp.query.length === 0) mailApp.loadList(false) })
  }

  function refreshAll() {
    loadFolders()
    loadList(false)
    flash("Refreshed")
  }

  // ---- reading -------------------------------------------------------------
  function select(index) {
    if (index < 0 || index >= messages.count) return
    cursor = index
    selectedUid = messages.get(index).uid
    openTimer.restart()
  }

  function moveCursor(step) {
    if (messages.count === 0) return
    var next = cursor < 0 ? 0 : Math.max(0, Math.min(messages.count - 1, cursor + step))
    if (next !== cursor) select(next)
    if (next >= messages.count - 3) loadMore()
  }

  function openCurrent(images) {
    if (cursor < 0 || cursor >= messages.count) return
    fetchMessage(folder, messages.get(cursor).uid, images === true)
  }

  // From a notification click: show that folder's list with the mail open.
  function showMessage(path, uid, force) {
    if (force || path !== folder || messages.count === 0) {
      folder = path
      setQuery("")
      loadList(true)
    }
    if (uid) {
      selectedUid = uid
      syncCursor()
      fetchMessage(path, uid, false)
    }
  }

  function rowFor(path, uid) {
    return path === folder ? indexOfUid(uid) : -1
  }

  function fetchMessage(path, uid, images) {
    var token = ++viewToken
    loadingView = true
    viewError = ""
    if (!images) view = null
    var before = rowFor(path, uid)
    var mark = before < 0 || !messages.get(before).seen
    backend.request("open", { folder: path, uid: uid, images: images, mark: mark }, function(ok, data, code) {
      if (token !== viewToken) return
      loadingView = false
      if (!ok) {
        viewError = data
        return handleError(data, code)
      }
      view = data
      viewFolder = path
      var index = rowFor(path, uid)
      currentFlagged = index >= 0 ? messages.get(index).flagged : false
      if (!images && index >= 0 && !messages.get(index).seen) {
        messages.setProperty(index, "seen", true)
        adjustUnread(path, -1)
      }
    })
  }

  function indexOfUid(uid) {
    for (var i = 0; i < messages.count; i++) if (messages.get(i).uid === uid) return i
    return -1
  }

  // The message actions apply to: the one open in the reader, else the
  // selected row. Never just "whatever row the cursor is on".
  function target() {
    if (view) return { folder: viewFolder, uid: view.uid }
    if (cursor >= 0 && cursor < messages.count) return { folder: folder, uid: messages.get(cursor).uid }
    return null
  }

  function loadImages() {
    if (view && view.remote_images > 0 && !view.images_loaded && !loadingView) {
      flash("Loading images…")
      fetchMessage(viewFolder, view.uid, true)
    }
  }

  function toggleSeen() {
    var t = target()
    if (!t) return
    var i = rowFor(t.folder, t.uid)
    var on = i >= 0 ? !messages.get(i).seen : false
    backend.request("flag", { folder: t.folder, uids: [t.uid], flag: "seen", on: on }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      var index = rowFor(t.folder, t.uid)
      if (index >= 0 && messages.get(index).seen !== on) {
        messages.setProperty(index, "seen", on)
        adjustUnread(t.folder, on ? -1 : 1)
      }
      flash(on ? "Marked as read" : "Marked as unread")
    })
  }

  function toggleStar() {
    var t = target()
    if (!t) return
    var i = rowFor(t.folder, t.uid)
    var on = i >= 0 ? !messages.get(i).flagged : !currentFlagged
    backend.request("flag", { folder: t.folder, uids: [t.uid], flag: "flagged", on: on }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      var index = rowFor(t.folder, t.uid)
      if (index >= 0) messages.setProperty(index, "flagged", on)
      if (view && view.uid === t.uid) currentFlagged = on
    })
  }

  function moveCurrent(role) {
    var t = target()
    if (!t) return
    var fromRole = roleOf(t.folder)
    if (role === "all" && fromRole !== "inbox" && fromRole !== "") return
    if (role === "trash" && fromRole === "trash") return
    if (role === "inbox" && fromRole !== "trash" && fromRole !== "junk") return
    var i = rowFor(t.folder, t.uid)
    var wasUnseen = i >= 0 && !messages.get(i).seen
    backend.request("move", { folder: t.folder, uids: [t.uid], to: role }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      if (view && view.uid === t.uid && viewFolder === t.folder) view = null
      var index = rowFor(t.folder, t.uid)
      if (index >= 0) {
        messages.remove(index)
        total = Math.max(0, total - 1)
        if (wasUnseen) adjustUnread(t.folder, -1)
        if (selectedUid === t.uid) {
          selectedUid = -1
          select(Math.min(index, messages.count - 1))
        } else {
          syncCursor()
        }
      }
      flash(role === "trash" ? "Moved to Trash" : role === "all" ? "Archived" : "Moved to Inbox")
    })
  }

  function saveAttachment(index, openAfter) {
    if (!view) return
    backend.request("attachment", { folder: viewFolder, uid: view.uid, index: index, open: openAfter }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      if (openAfter && !data.opened) flash("Saved " + data.path + " (not opened: this file type could run code)")
      else flash("Saved " + data.path)
    })
  }

  function openLink(link) {
    var kind = Fmt.linkKind(link)
    if (kind === "web") Qt.openUrlExternally(link)
    else if (kind === "mail") compose("new", Fmt.mailtoAddress(link))
  }

  // ---- composing -----------------------------------------------------------
  function compose(mode, to) {
    if (draft) return
    sendError = ""
    if (mode === "new") {
      draft = { to: to || "", cc: "", subject: "", body: "", in_reply_to: "", references: "",
                attachments: [], forward_folder: "", forward_uid: 0 }
      return
    }
    if (!view) return
    var uid = view.uid
    var source = viewFolder
    backend.request("draft", { folder: source, uid: uid, mode: mode }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      data.forward_folder = mode === "forward" ? source : ""
      data.forward_uid = mode === "forward" ? uid : 0
      draft = data
    })
  }

  // The result lands here, not in the composer, so it is reported even if
  // the composer is gone by then.
  function sendDraft(message) {
    if (sending) return
    sending = true
    sendError = ""
    backend.request("send", message, function(ok, data) {
      sending = false
      if (!ok) {
        sendError = data
        flash("Not sent: " + data)
        return
      }
      draft = null
      flash("Sent")
      Qt.callLater(focusList)
    })
  }

  function discardCompose() {
    if (sending) {
      flash("Still sending…")
      return
    }
    var composer = composerLoader.item
    if (composer && composer.dirty) {
      askConfirm("Discard this message?", "Discard", function() {
        draft = null
        Qt.callLater(focusList)
      })
    } else {
      draft = null
      Qt.callLater(focusList)
    }
  }

  // ---- confirm -------------------------------------------------------------
  function askConfirm(message, confirmText, action) {
    confirmDialog.message = message
    confirmDialog.confirmText = confirmText
    confirmDialog.selectedIndex = 1
    confirmAction = action
    confirming = true
    keySink.forceActiveFocus()
  }

  function finishConfirm(accepted) {
    var action = confirmAction
    confirming = false
    confirmAction = null
    if (accepted && action) action()
    else if (composerLoader.item) composerLoader.item.focusFirst()
    else focusList()
  }

  // ---- focus + keys --------------------------------------------------------
  function listView() {
    return listLoader.item
  }

  function focusList() {
    if (phase === "mail" && !draft) keySink.forceActiveFocus()
  }

  function handleKey(event) {
    if (confirming) return confirmDialog.handleKey(event)
    if (phase !== "mail" || draft) {
      if (event.key === Qt.Key_Escape && phase !== "mail") { requestClose(); return true }
      return false
    }
    // Leave Ctrl/Alt/Super combinations to the system and text fields.
    if (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)) return false
    var shift = event.modifiers & Qt.ShiftModifier
    switch (event.key) {
    case Qt.Key_J:
      if (shift) selectFolderBy(1); else moveCursor(1)
      return true
    case Qt.Key_Down: moveCursor(1); return true
    case Qt.Key_K:
      if (shift) selectFolderBy(-1); else moveCursor(-1)
      return true
    case Qt.Key_Up: moveCursor(-1); return true
    case Qt.Key_Return:
    case Qt.Key_Enter: openCurrent(false); return true
    case Qt.Key_Space:
      if (readerLoader.item) readerLoader.item.scrollBy(shift ? -300 : 300)
      return true
    case Qt.Key_PageDown:
      if (readerLoader.item) readerLoader.item.scrollBy(300)
      return true
    case Qt.Key_PageUp:
      if (readerLoader.item) readerLoader.item.scrollBy(-300)
      return true
    case Qt.Key_Slash:
      if (listView()) listView().searchField.forceActiveFocus()
      return true
    case Qt.Key_C: compose("new"); return true
    case Qt.Key_R:
      if (shift) refreshAll(); else compose("reply")
      return true
    case Qt.Key_A: compose("reply_all"); return true
    case Qt.Key_F: compose("forward"); return true
    case Qt.Key_E: moveCurrent("all"); return true
    case Qt.Key_NumberSign:
    case Qt.Key_Delete: moveCurrent("trash"); return true
    case Qt.Key_S: toggleStar(); return true
    case Qt.Key_U: toggleSeen(); return true
    case Qt.Key_I: loadImages(); return true
    case Qt.Key_Escape:
      if (query.length > 0) search("")
      else requestClose()
      return true
    }
    return false
  }

  Timer {
    id: openTimer
    interval: 180
    onTriggered: mailApp.openCurrent(false)
  }

  Timer {
    id: statusTimer
    interval: 4000
    onTriggered: mailApp.status = ""
  }

  // Labels get no push from the server (IDLE watches INBOX only), and a
  // missed push must not leave the list stale for long either.
  Timer {
    interval: 3 * 60 * 1000
    repeat: true
    running: mailApp.phase === "mail"
    onTriggered: {
      mailApp.loadFolders()
      if (mailApp.query.length === 0 && !mailApp.loadingList) mailApp.loadList(false, true)
    }
  }


  // ---- window --------------------------------------------------------------
  FloatingWindow {
    id: window
    title: "Omamail"
    color: Color.background
    implicitWidth: 1180
    implicitHeight: 760
    minimumSize: Qt.size(820, 520)
    visible: false

    onVisibleChanged: {
      if (!visible && !mailApp.closingFromHost) mailApp.requestClose()
    }

    FocusScope {
      id: keyScope
      anchors.fill: parent
      focus: true

      // A plain Item, not a scope: forceActiveFocus() on it really moves
      // focus away from the search field or composer, which calling it on
      // the enclosing FocusScope would not.
      Item {
        id: keySink
        focus: true
        Keys.onPressed: function(event) {
          if (mailApp.handleKey(event)) event.accepted = true
        }
      }

      // Keys a focused child (e.g. the reader after a click) did not use.
      Keys.onPressed: function(event) {
        if (mailApp.handleKey(event)) event.accepted = true
      }

      Text {
        textFormat: Text.PlainText
        anchors.centerIn: parent
        visible: mailApp.phase === "loading"
        text: "Omamail"
        color: Qt.darker(Color.foreground, 1.6)
        font.family: Style.font.family
        font.pixelSize: Style.font.display
      }

      Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - Style.space(40), Style.space(520))
        visible: mailApp.phase === "setup"
        spacing: Style.space(12)

        Text {
          textFormat: Text.PlainText
          text: "One more step"
          color: Color.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.display
          font.bold: true
        }
        Text {
          textFormat: Text.PlainText
          width: parent.width
          wrapMode: Text.WordWrap
          text: "The mail backend has not been built yet. Run this in a terminal (it needs Rust: omarchy install dev-env rust), then open Omamail again:"
          color: Qt.darker(Color.foreground, 1.4)
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
        }
        TextField {
          width: parent.width
          readOnly: true
          selectByMouse: true
          text: mailApp.installScript
        }
      }

      Loader {
        anchors.fill: parent
        active: mailApp.phase === "signin"
        sourceComponent: SignIn { app: mailApp }
      }

      Item {
        id: mailView
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: statusBar.top
        visible: mailApp.phase === "mail"

        Sidebar {
          id: sidebar
          app: mailApp
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          anchors.left: parent.left
          width: Style.space(190)
        }

        Rectangle {
          id: sidebarRule
          anchors.left: sidebar.right
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          width: 1
          color: Util.alpha(Color.foreground, 0.12)
        }

        Loader {
          id: listLoader
          anchors.left: sidebarRule.right
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          width: Math.max(Style.space(300), Math.min(Style.space(380), mailView.width * 0.32))
          active: mailApp.phase === "mail"
          sourceComponent: MessageList { app: mailApp }
        }

        Rectangle {
          id: listRule
          anchors.left: listLoader.right
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          width: 1
          color: Util.alpha(Color.foreground, 0.12)
        }

        Loader {
          id: readerLoader
          anchors.left: listRule.right
          anchors.right: parent.right
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          active: mailApp.phase === "mail"
          visible: !mailApp.draft
          sourceComponent: Reader { app: mailApp }
        }

        Loader {
          id: composerLoader
          anchors.fill: readerLoader
          active: mailApp.draft !== null
          sourceComponent: Composer { app: mailApp; draft: mailApp.draft }
        }
      }

      Rectangle {
        id: statusBar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: mailApp.phase === "mail" ? Style.space(24) : 0
        visible: mailApp.phase === "mail"
        color: Util.alpha(Color.foreground, 0.03)

        Rectangle {
          anchors.top: parent.top
          width: parent.width
          height: 1
          color: Util.alpha(Color.foreground, 0.12)
        }

        Text {
          textFormat: Text.PlainText
          id: hints
          anchors.left: parent.left
          anchors.leftMargin: Style.space(12)
          anchors.verticalCenter: parent.verticalCenter
          color: Qt.darker(Color.foreground, 1.6)
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: mailApp.draft
            ? "Ctrl+Enter send · Tab next field · Esc discard"
            : "j/k move · J/K folder · / search · c compose · r reply · a all · f forward · e archive · # delete · s star · u unread"
        }

        Text {
          textFormat: Text.PlainText
          anchors.right: parent.right
          anchors.rightMargin: Style.space(12)
          anchors.left: hints.right
          anchors.leftMargin: Style.space(16)
          anchors.verticalCenter: parent.verticalCenter
          horizontalAlignment: Text.AlignRight
          elide: Text.ElideMiddle
          color: mailApp.hoveredLink.length > 0 ? Color.accent : Color.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: mailApp.hoveredLink.length > 0 ? mailApp.hoveredLink : mailApp.status
        }
      }

      ConfirmDialog {
        id: confirmDialog
        anchors.fill: parent
        opened: mailApp.confirming
        onCanceled: mailApp.finishConfirm(false)
        onConfirmed: mailApp.finishConfirm(true)
      }
    }
  }
}
