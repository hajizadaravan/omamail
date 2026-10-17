<p align="center"><img src="share/omamail.svg" width="96" alt="Omamail"></p>

# Omamail

A light Gmail client that lives inside the [Omarchy](https://omarchy.org) shell.

- **UI**: a QML panel plugin for `omarchy-shell`. It uses the shell's own
  components and theme tokens, so it follows the current Omarchy theme,
  font and rounding. Nothing runs while the window is closed: the shell
  unloads the plugin, which ends the backend.
- **Backend**: a ~3 MB Rust binary (`omamail`), about 7 MB of RAM. IMAP for
  reading, SMTP for sending, all over TLS.
- **Notifications**: `omamail watch` runs as a user service, sits in IMAP
  IDLE on the inbox and posts a desktop notification per new message.
  Clicking it opens the message.

## Install

Needs Rust (`cargo`), `secret-tool` (libsecret) with a running keyring, and
`zenity` for the attachment picker.

```bash
./install.sh
```

This builds the backend, links `plugin/` to
`~/.config/omarchy/plugins/reavann.omamail`, installs a launcher entry and
the `omamail-watch` user service, and enables the plugin.

Open it from the app launcher ("Omamail"), or bind a key in
`~/.config/hypr/bindings.lua` to:

```bash
omarchy-shell shell toggle reavann.omamail '{}'
```

### Signing in

Omamail uses a Gmail **app password**: turn on 2-Step Verification, then
create one at <https://myaccount.google.com/apppasswords>. The password is
stored in the desktop keyring (`secret-tool`), never in a file; the address
and server names live in `~/.config/omamail/config.json`.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, arrows | next / previous message (opens it) |
| `J` / `K` | next / previous folder |
| `Enter` | open the selected message |
| `Space` / `Shift+Space` | scroll the message |
| `/` | search (Gmail syntax works: `from:x has:attachment`) |
| `c` | compose |
| `r` / `a` / `f` | reply / reply all / forward |
| `e` | archive |
| `#`, `Delete` | move to Trash |
| `s` | star |
| `u` | toggle read |
| `i` | load remote images |
| `R` | refresh |
| `Esc` | clear search, then close |
| `Ctrl+Enter` | send (composer) |

## Privacy and safety

- Mail HTML is sanitized (ammonia) down to the small subset Qt renders:
  no scripts, forms, iframes, inline styles or event handlers. Links open
  only for `http(s)` and `mailto`; the target shows in the status bar on hover.
- Remote images are blocked until you press **Load images**, so opening a
  mail does not ping trackers. When loaded, they are fetched by the backend
  from public addresses only (no LAN, localhost or Tailscale targets),
  at most 40 images of 5 MB each.
- Attachments are saved to `~/Downloads` under a sanitized file name and
  never overwrite an existing file. "Save and open" only opens common
  document, image and media types, checked by extension *and* by content
  (a JAR named `invoice.pdf` is saved, not run).
- All other mail text (subjects, names) is shown as plain text, never markup.

## Development

```
backend/   Rust: IMAP/SMTP, MIME, sanitizer, JSON-lines server, watcher
plugin/    QML panel plugin (Panel.qml is the entry point)
share/     launcher entry and user service unit
```

```bash
cd backend && cargo test     # unit tests
./install.sh                 # rebuild + reinstall
omarchy restart shell        # pick up QML changes
```

The shell caches compiled QML, and its file watcher does not follow the
plugin symlink, so QML edits need a shell restart to show up.

The backend speaks one JSON object per line on stdio
(`{"id":1,"cmd":"list","folder":"INBOX","before_uid":0}`); see `backend/src/server.rs`.
