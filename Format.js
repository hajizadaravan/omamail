.pragma library

// Nerd Font (Material Design) glyphs, same family the Omarchy bar uses.
var icons = {
  inbox: "\u{f0687}",
  flagged: "\u{f04ce}",
  starOff: "\u{f04d2}",
  sent: "\u{f048a}",
  drafts: "\u{f03eb}",
  all: "\u{f003c}",
  junk: "\u{f0028}",
  trash: "\u{f01b4}",
  label: "\u{f0315}",
  compose: "\u{f03eb}",
  reply: "\u{f045a}",
  replyAll: "\u{f045b}",
  forward: "\u{f0496}",
  archive: "\u{f003c}",
  attachment: "\u{f03e2}",
  search: "\u{f0349}",
  refresh: "\u{f0450}",
  unread: "\u{f01ee}",
  read: "\u{f01ef}",
  image: "\u{f02e9}",
  download: "\u{f01da}",
  open: "\u{f03cc}",
  logout: "\u{f0343}",
  about: "\u{f02fd}",
  close: "\u{f0156}"
}

function folderIcon(role) {
  return icons[role] || icons.label
}

function person(p) {
  if (!p) return ""
  return p.name && p.name.length > 0 ? p.name : (p.email || "")
}

function people(list) {
  if (!list || list.length === 0) return ""
  var out = []
  for (var i = 0; i < list.length; i++) out.push(person(list[i]))
  return out.join(", ")
}

function fullPeople(list) {
  if (!list || list.length === 0) return ""
  var out = []
  for (var i = 0; i < list.length; i++) {
    var p = list[i]
    out.push(p.name && p.name.length > 0 ? p.name + " <" + p.email + ">" : p.email)
  }
  return out.join(", ")
}

function shortDate(ts) {
  if (!ts) return ""
  var d = new Date(ts * 1000)
  var now = new Date()
  if (d.toDateString() === now.toDateString()) return Qt.formatTime(d, "HH:mm")
  if (d.getFullYear() === now.getFullYear()) return Qt.formatDate(d, "d MMM")
  return Qt.formatDate(d, "dd.MM.yyyy")
}

function longDate(ts) {
  if (!ts) return ""
  return Qt.formatDateTime(new Date(ts * 1000), "ddd, d MMM yyyy  HH:mm")
}

function size(bytes) {
  if (bytes < 1024) return bytes + " B"
  if (bytes < 1024 * 1024) return Math.round(bytes / 1024) + " KB"
  return (bytes / 1024 / 1024).toFixed(1) + " MB"
}

function cssColor(c) {
  function hex(v) {
    var s = Math.round(v * 255).toString(16)
    return s.length < 2 ? "0" + s : s
  }
  return "#" + hex(c.r) + hex(c.g) + hex(c.b)
}

// Only web and mail links leave the reader; everything else is ignored.
function linkKind(url) {
  var u = String(url || "").toLowerCase()
  if (u.indexOf("https://") === 0 || u.indexOf("http://") === 0) return "web"
  if (u.indexOf("mailto:") === 0) return "mail"
  return ""
}

function mailtoAddress(url) {
  var rest = String(url).substring(7)
  var q = rest.indexOf("?")
  if (q >= 0) rest = rest.substring(0, q)
  try { return decodeURIComponent(rest) } catch (e) { return rest }
}
