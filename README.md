<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="share/logo-white.svg">
    <img src="share/logo-black.svg" width="96" alt="Omamail">
  </picture>
</p>

# Omamail

A light Gmail client that lives inside the [Omarchy](https://omarchy.org) shell.

![Omamail in the Omarchy shell, following the current theme](share/screenshots/omamail.png)

<sub>Shown with sample mail.</sub>

- **UI**: a QML panel plugin for `omarchy-shell`. It uses the shell's own
  components and theme tokens, so it follows the current Omarchy theme,
  font and rounding. Nothing runs while the window is closed: the shell
  unloads the plugin, which ends the backend.
- **Backend**: a ~3 MB Rust binary (`omamail`), about 7 MB of RAM. IMAP for
  reading, SMTP for sending, all over TLS.
- **Notifications**: `omamail watch` runs as a user service, sits in IMAP
  IDLE on the inbox and posts a desktop notification per new message.
  Clicking it opens the message.
- **Verification codes**: when a new mail carries a one-time code ("295092 is
  your code", "Doğrulama kodunuz: 123 456"), the notification shows the code
  and clicking it copies the code to the clipboard instead. Codes are read
  from the subject, or from the body when the subject reads like a
  verification mail. The copy is marked sensitive, so it stays out of the
  clipboard history; the code does show in Omarchy's notification history.
- **Updates**: once a day a user timer checks the plugin's git checkout for
  new commits and, when there are some, posts a notification listing them.
  Clicking it opens a terminal that shows the diff and installs only what
  you accept. Nothing is installed without you.

## Install

Omamail is an Omarchy shell plugin. Add it like any other plugin, then
build its backend once:

```bash
omarchy plugin add https://github.com/reav4nn/omamail.git --enable
~/.config/omarchy/plugins/reavann.omamail/install.sh
```

`omarchy plugin add` only clones the repo; `install.sh` builds the Rust
backend into `~/.local/lib/omamail/` and installs the launcher entry, the icon and the
`omamail-watch` notification service. It needs Rust
(`omarchy install dev-env rust`); attaching files needs `zenity`
(`omarchy pkg add zenity`). See [Requirements and permissions](#requirements-and-permissions).

Open it from the app launcher ("Omamail"), or bind a key in
`~/.config/hypr/bindings.lua` to:

```bash
omarchy-shell shell toggle reavann.omamail '{}'
```

To update, click the "update available" notification, or run:

```bash
~/.config/omarchy/plugins/reavann.omamail/update.sh
```

It runs `omarchy plugin update`, which shows the diff first: look over
changes to `backend/` and `install.sh` before accepting, since `install.sh`
builds that code. Only after you accept does it rebuild (`install.sh`) and
restart the shell. The daily check itself only fetches, into
`~/.cache/omamail/update.git`, never into the plugin folder; to turn it off:

```bash
systemctl --user disable --now omamail-update-check.timer
```

`install.sh` enables the timer only once, so it stays off.

To remove:

```bash
systemctl --user disable --now omamail-watch.service omamail-update-check.timer
omarchy plugin remove reavann.omamail
rm -rf ~/.local/lib/omamail ~/.config/omamail ~/.cache/omamail ~/.local/state/omamail
rm ~/.config/systemd/user/omamail-watch.service ~/.config/systemd/user/omamail-update-check.{service,timer} \
   ~/.local/share/applications/omamail.desktop \
   ~/.local/share/icons/hicolor/scalable/apps/omamail.svg
secret-tool clear application omamail
```

### Signing in

Omamail uses a Gmail **app password**: turn on 2-Step Verification, then
create one at <https://myaccount.google.com/apppasswords>. The password is
stored in the desktop keyring (`secret-tool`), never in a file; the address
and server names live in `~/.config/omamail/config.json`.

## Requirements and permissions

What Omamail needs, and everything it touches outside the plugin folder:

- **Build**: Rust (`cargo`). `install.sh` builds `backend/` with `--locked`;
  build output goes to `~/.cache/omamail/target`.
- **Runtime**: `secret-tool` with a running keyring (gnome-keyring),
  `wl-copy` and `xdg-open`, which Omarchy ships, and `zenity` for the
  attachment picker, which it does not: `omarchy pkg add zenity`.
- **Network**: `imap.gmail.com:993` and `smtp.gmail.com:465` over TLS. Remote
  images are fetched only after you press *Load images*, and only from public
  addresses; like in any mail client, the sender's server then sees your IP,
  and `http://` images travel unencrypted. Once a day the update check runs
  `git fetch` against the plugin's remote (GitHub).
- **Files**: the backend binary in `~/.local/lib/omamail/`; account settings
  in `~/.config/omamail/`; image cache and the update check's git cache in `~/.cache/omamail/`; watcher and
  update-check state in `~/.local/state/omamail/`; saved attachments in `$XDG_DOWNLOAD_DIR` or
  `~/Downloads`. A launcher entry and icon go to `~/.local/share/`, the unit
  files to `~/.config/systemd/user/` (`omamail-watch.service`,
  `omamail-update-check.service` and `.timer`).
- **Services**: `omamail-watch.service`, a systemd user unit that keeps an
  IMAP IDLE connection open for new-mail notifications, and
  `omamail-update-check.timer`, which runs the daily update check. No root and no sudo;
  besides enabling the plugin, nothing in your existing configuration changes.
- **Secrets**: the Gmail app password lives only in the keyring; it never
  goes to argv, a file or a log. Notifications are sent straight to
  `org.freedesktop.Notifications` over the session D-Bus socket and
  verification codes are piped to `wl-copy`, so mail senders, subjects and
  codes never appear in process arguments.
- **Signing out** removes the password from the keyring, the account
  settings and the watcher state; cached images stay until you remove them
  (see *To remove* above).

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
manifest.json, Panel.qml, Backend.qml, Format.js, components/
           the QML panel plugin (Panel.qml is the entry point)
backend/   Rust: IMAP/SMTP, MIME, sanitizer, JSON-lines server, watcher
share/     launcher entry, icon, logo, user service and timer units
install.sh, update.sh
           build + install; update (diff, then install.sh)
```

Working from a checkout elsewhere, `./install.sh` builds the backend and
copies the plugin files (without `.git`) into
`~/.config/omarchy/plugins/reavann.omamail`; run it again after each change.
It never links the checkout there: `omarchy plugin update` would treat a
linked checkout as its own and may reset it.

```bash
cd backend && cargo test     # unit tests
./install.sh                 # rebuild + reinstall
omarchy restart shell        # pick up QML changes
```

The shell caches compiled QML, so QML edits need a shell restart to show up.

The backend speaks one JSON object per line on stdio
(`{"id":1,"cmd":"list","folder":"INBOX","before_uid":0}`); see `backend/src/server.rs`.
