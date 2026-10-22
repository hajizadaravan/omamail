# AGENTS.md

Omamail: a Gmail client plugin for the Omarchy shell. QML panel
(`Panel.qml`, `components/`) talking JSON lines to a Rust backend
(`backend/`), plus a `omamail watch` user service for notifications.

## Commands

```bash
cd backend && cargo test --locked && cargo clippy --locked   # before every commit
./install.sh             # build + install backend, units, and the plugin files
omarchy restart shell    # reload QML
```

Run from a dev checkout, `install.sh` copies the plugin into
`~/.config/omarchy/plugins/reavann.omamail`, and refuses (exit 1) when that
folder is a git checkout from `omarchy plugin add`. Never symlink a checkout
there: `omarchy plugin update` may `git reset --hard` it.

## Rules

- **Bump the version with every change on `main`** (semver), in
  `manifest.json`, `backend/Cargo.toml` and `backend/Cargo.lock` together.
  See CONTRIBUTING.md, "Versioning".
- **Builds are `--locked`.** Never run a plain `cargo update`; add crates
  only when necessary, with the lockfile change in the same commit.
- **The app password stays in the keyring.** Never put it, mail text or
  verification codes in argv, logs or files (codes go to the 0600 state
  file, `wl-copy` via stdin, notifications over D-Bus directly).
- **Mail is hostile input.** HTML only through `sanitize.rs`; subjects and
  names are plain text in QML and markup-escaped in notification bodies
  (the summary is shown as plain text: don't escape it). Remote images:
  public IPs only, no redirects, size/count caps (`images.rs`).
  Attachments: never overwrite, open only allowlisted types checked by
  extension and content (`server.rs`). Links: `http(s)` and `mailto` only.
  Details: README, "Privacy and safety".
- **imap 2.4 quirks:** a connection is never reused after IDLE, and one
  that hit a connection error is dropped (`poison`), or a late reply trips
  the tag assertion.
- **Gmail is the latency floor:** it announces new mail over IMAP 10–50 s
  after delivery (measured; Thunderbird is the same). Don't chase 1–2 s.
- Tests must not need a real account or Internet access.
- Commits: Conventional Commits, no AI attribution or `Co-Authored-By`.
