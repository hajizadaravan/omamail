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
  property bool loadingMore: false
  property string listError: ""
  property string query: ""
  // The selection is tracked by UID; `cursor` is only its current row.
  property int selectedUid: -1
  property int cursor: -1
  property int listToken: 0
  property int listValidity: 0
  // False until this session's list reply confirms the mailbox. Saved rows
  // are visible before that, but changing them (star, move) waits until confirmed.
  property bool listLive: false
  property bool refreshPending: false

  property var view: null
  property string viewFolder: ""
  // UIDVALIDITY the open message was fetched under. Later actions send this,
  // not whatever the list learned afterwards.
  property int viewValidity: 0
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
  // The backend's id for the last move while it can be undone (z), else 0.
  property int undoId: 0
  // A message to select once the next list reply for `pendingSelectFolder`
  // has it (after an undo).
  property int pendingSelectUid: -1
  property string pendingSelectFolder: ""
  property int pendingSelectValidity: 0
  property bool aboutOpen: false
  property string version: ""

  // `undo`: offer that move back for as long as the message shows.
  function flash(text, undo) {
    status = text
    undoId = undo || 0
    statusTimer.interval = undoId ? 8000 : 4000
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
      version = data.version || ""
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
      // Rows left by an auth error may be another account's: start empty,
      // or selectFolder would refresh them in place as if confirmed.
      messages.clear()
      view = null
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
    // Re-selecting the open folder refreshes in place. A different folder,
    // or the first paint, starts from the saved headers.
    var keep = path === folder && query.length === 0 && messages.count > 0
    folder = path
    setQuery("")
    dropView()
    loadList(!keep)
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

  // A plain copy of a model row, to merge with new rows.
  function rowAt(i) {
    var r = messages.get(i)
    return { uid: r.uid, fromName: r.fromName, fromEmail: r.fromEmail, subject: r.subject,
      timestamp: r.timestamp, seen: r.seen, flagged: r.flagged, attachment: r.attachment }
  }

  // The lowest and highest UID loaded. Rows are shown by date, but pages
  // are fetched by UID, so "older" and "newer" are asked of these.
  function uidRange(rows) {
    var lo = -1, hi = -1
    for (var i = 0; i < rows.length; i++) {
      if (lo < 0 || rows[i].uid < lo) lo = rows[i].uid
      if (rows[i].uid > hi) hi = rows[i].uid
    }
    return { lo: lo, hi: hi }
  }

  function loadedRows() {
    var rows = []
    for (var i = 0; i < messages.count; i++) rows.push(rowAt(i))
    return rows
  }

  // Updates the model in place, so a refresh keeps the scroll position and
  // only repaints rows that changed. Newest first by arrival time, like
  // Gmail: mail moved back into a folder gets a new UID but keeps its date.
  function applyRows(rows) {
    rows.sort(function(a, b) { return b.timestamp - a.timestamp || b.uid - a.uid })
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

  // reset: a different folder or query. Saved headers paint first, then the
  // server page replaces them. A refresh (reset false) keeps older rows
  // loaded in this session when UIDVALIDITY still matches. quiet: a
  // background refresh, whose failure is not worth a message.
  function loadList(reset, quiet) {
    var token = ++listToken
    var fromServer = false
    if (reset) {
      pendingSelectUid = -1
      messages.clear()
      total = 0
      selectedUid = -1
      cursor = -1
      listValidity = 0
      listLive = false
    }
    loadingMore = false
    // A background refresh keeps a shown error until it succeeds.
    if (!quiet) listError = ""
    loadingList = true
    var searching = query.length > 0
    if (reset && !searching) {
      backend.request("cached", { folder: folder }, function(ok, data) {
        if (token !== listToken || fromServer) return
        if (!ok || !data || !data.messages || data.messages.length === 0) return
        var rows = []
        for (var i = 0; i < data.messages.length; i++) rows.push(flatten(data.messages[i]))
        applyRows(rows)
        total = data.total || rows.length
        if (data.uidvalidity) listValidity = data.uidvalidity
        syncCursor()
      })
    }
    backend.request(searching ? "search" : "list", { folder: folder, query: query }, function(ok, data, code) {
      if (token !== listToken) return
      fromServer = true
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
      // Saved rows are not scrollback. Keep older rows only when this
      // session already confirmed the mailbox, and the new page reaches them.
      var shownValidity = listValidity
      var confirmed = listLive
      var oldest = uidRange(rows).lo
      var overlaps = rows.length > 0 && messages.count > 0 && oldest <= uidRange(loadedRows()).hi
      var sameMailbox = shownValidity !== 0 && data.uidvalidity === shownValidity
      if (!searching && !reset && confirmed && overlaps && sameMailbox) {
        for (var j = 0; j < messages.count; j++) {
          if (messages.get(j).uid < oldest) rows.push(rowAt(j))
        }
      }
      applyRows(rows)
      total = data.total
      if (data.uidvalidity) listValidity = data.uidvalidity
      listLive = true
      syncCursor()
      if (pendingSelectUid >= 0 && !searching) {
        // Only in the mailbox the UID came from: a rebuilt one reuses UIDs.
        var restored = pendingSelectFolder === folder && data.uidvalidity === pendingSelectValidity
          ? indexOfUid(pendingSelectUid) : -1
        pendingSelectUid = -1
        if (restored >= 0) select(restored)
      }
    })
  }

  function loadMore() {
    if (!listLive || loadingList || query.length > 0 || messages.count === 0 || messages.count >= total) return
    loadingList = true
    loadingMore = true
    var token = ++listToken
    var before = uidRange(loadedRows()).lo
    backend.request("list", { folder: folder, before_uid: before }, function(ok, data, code) {
      if (token !== listToken) return
      loadingList = false
      loadingMore = false
      runPendingRefresh()
      if (!ok) return handleError(data, code)
      if (data.uidvalidity && listValidity !== 0 && data.uidvalidity !== listValidity) {
        return loadList(true)
      }
      var rows = loadedRows()
      for (var i = 0; i < data.messages.length; i++) {
        var m = data.messages[i]
        if (indexOfUid(m.uid) < 0) rows.push(flatten(m))
      }
      applyRows(rows)
      syncCursor()
      total = data.total
    })
  }

  // Closes the reader, and an open still on its way must not reopen it.
  function dropView() {
    ++viewToken
    loadingView = false
    view = null
    viewError = ""
  }

  function search(text) {
    setQuery(text.trim())
    dropView()
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
    openTimer.stop()
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

  // UIDVALIDITY to send with an action. An open message keeps the generation
  // it was fetched under; a row uses the list the server just confirmed.
  // Zero means "not confirmed", and the backend then skips the check.
  function generationFor(path, uid) {
    if (view && view.uid === uid && viewFolder === path && viewValidity) return viewValidity
    return path === folder ? listValidity : 0
  }

  // Saved rows look final, but archive, trash, and flags must wait until
  // the server list has confirmed them. An open message was fetched live,
  // but its seen and starred state still comes from the row: a toggle
  // (usesFlags) waits even then, or it would flip the saved value.
  // A list that failed is asked for again, or the wait would never end.
  function refuseStaleRow(usesFlags) {
    if (listLive || (view && !usesFlags)) return false
    if (!loadingList) loadList(false)
    flash("Updating the list…")
    return true
  }

  function fetchMessage(path, uid, images) {
    var token = ++viewToken
    loadingView = true
    viewError = ""
    var validity = generationFor(path, uid)
    if (!images) {
      view = null
      viewValidity = 0
    }
    var before = rowFor(path, uid)
    // A saved row may say "seen" for mail marked unread since.
    var mark = before < 0 || !listLive || !messages.get(before).seen
    backend.request("open", { folder: path, uid: uid, images: images, mark: mark, uidvalidity: validity }, function(ok, data, code) {
      if (token !== viewToken) return
      loadingView = false
      if (!ok) {
        viewError = data
        return handleError(data, code)
      }
      view = data
      viewFolder = path
      viewValidity = data.uidvalidity || validity
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
    if (!t || refuseStaleRow(true)) return
    var i = rowFor(t.folder, t.uid)
    var on = i >= 0 ? !messages.get(i).seen : false
    backend.request("flag", { folder: t.folder, uids: [t.uid], flag: "seen", on: on, uidvalidity: generationFor(t.folder, t.uid) }, function(ok, data, code) {
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
    if (!t || refuseStaleRow(true)) return
    var i = rowFor(t.folder, t.uid)
    var on = i >= 0 ? !messages.get(i).flagged : !currentFlagged
    backend.request("flag", { folder: t.folder, uids: [t.uid], flag: "flagged", on: on, uidvalidity: generationFor(t.folder, t.uid) }, function(ok, data, code) {
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
    if (refuseStaleRow(false)) return
    var i = rowFor(t.folder, t.uid)
    var wasUnseen = i >= 0 && !messages.get(i).seen
    backend.request("move", { folder: t.folder, uids: [t.uid], to: role, uidvalidity: generationFor(t.folder, t.uid) }, function(ok, data, code) {
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
      var done = role === "trash" ? "Moved to Trash" : role === "all" ? "Archived" : "Moved to Inbox"
      var undo = data && data.undo ? data.undo : 0
      flash(undo ? done + " · z to undo" : done, undo)
    })
  }

  // The moved message gets a new UID where it lands, so the list is asked
  // for again rather than patched.
  // Not over the composer: its Esc and Ctrl+Enter shortcuts would still fire.
  function showAbout() {
    if (phase !== "mail" || draft) return
    aboutOpen = true
    keySink.forceActiveFocus()
  }

  function undoLast() {
    if (!undoId) return
    var id = undoId
    flash("Undoing…")
    backend.request("undo", { undo: id }, function(ok, data, code) {
      // "retry": the backend kept the move after a network error.
      if (!ok && code === "retry") return flash(data + " · z to try again", id)
      if (!ok) return handleError(data, code)
      flash("Undone")
      loadFolders()
      // The reader may show it where it no longer is (a copy leaves it there).
      if (!data.copied && view && viewFolder === data.from && data.moved && data.moved.indexOf(view.uid) >= 0) dropView()
      if (data.folder !== folder && data.from !== folder) return
      // Back where it was: select it again, as before it was moved. Search
      // results are only refreshed.
      if (query.length === 0 && data.folder === folder && data.uid && data.uidvalidity) {
        pendingSelectUid = data.uid
        pendingSelectFolder = folder
        pendingSelectValidity = data.uidvalidity
      }
      loadList(false)
    })
  }

  function saveAttachment(index, openAfter) {
    if (!view) return
    backend.request("attachment", { folder: viewFolder, uid: view.uid, index: index, open: openAfter, uidvalidity: generationFor(viewFolder, view.uid) }, function(ok, data, code) {
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
    var validity = generationFor(source, uid)
    backend.request("draft", { folder: source, uid: uid, mode: mode, uidvalidity: validity }, function(ok, data, code) {
      if (!ok) return handleError(data, code)
      data.forward_folder = mode === "forward" ? source : ""
      data.forward_uid = mode === "forward" ? uid : 0
      data.uidvalidity = mode === "forward" ? validity : 0
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
    if (aboutOpen && phase === "mail") {
      if (event.key === Qt.Key_Escape || event.key === Qt.Key_Question || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) aboutOpen = false
      return true
    }
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
    case Qt.Key_Z: undoLast(); return true
    case Qt.Key_Question: showAbout(); return true
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
    onTriggered: {
      mailApp.status = ""
      mailApp.undoId = 0
    }
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

          MouseArea {
            anchors.fill: parent
            enabled: mailApp.undoId > 0
            cursorShape: Qt.PointingHandCursor
            onClicked: mailApp.undoLast()
          }
        }
      }

      About {
        anchors.fill: parent
        app: mailApp
        opened: mailApp.aboutOpen && mailApp.phase === "mail" && !mailApp.draft
        onClosed: {
          mailApp.aboutOpen = false
          mailApp.focusList()
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
