# AGENTS.md

Omamail: a Gmail client plugin for the Omarchy shell. QML panel
(`Panel.qml`, `components/`) talking JSON lines to a Rust backend
(`backend/`), plus a `omamail watch` user service for notifications.

## Commands

```bash
cd backend && cargo test --locked && cargo clippy --locked   # before every commit
cargo test --locked --features demo                          # if demo.rs or server.rs changed
./install.sh             # build + install backend, units, and the plugin files
omarchy restart shell    # reload QML
tools/make-preview.sh    # regenerate preview.png from the `demo` build (switches themes, restores them)
git push origin main && git tag v1.2.3 && git push origin v1.2.3   # marketplace update request
gh release create v1.2.3 --verify-tag --generate-notes --latest    # GitHub release, after every tag
```

The tag must equal `manifest.json`'s version and point at main's HEAD (the
marketplace accepts nothing else). `.github/workflows/marketplace.yml` then
files the marketplace's verification request; a marketplace maintainer still
approves it. The workflow does not create a GitHub Release (its token is
read-only); the `gh release create` step above does, so never skip it. It fails while the listing records an old repository path (after
an account or repository rename) until a maintainer records the migration. It needs `MARKETPLACE_TOKEN` in the `marketplace` environment (only `v*`
tags may deploy): a classic token with only `public_repo` and an expiry.
Fine-grained tokens and `GITHUB_TOKEN` cannot open issues in another owner's
repository; the classic token can write to every public repository of its
account, so never widen the environment rule or log the token.

Run from a dev checkout, `install.sh` copies the plugin into
`~/.config/omarchy/plugins/reavann.omamail`, and refuses (exit 1) when that
folder is a git checkout from `omarchy plugin add`. Never symlink a checkout
there: `omarchy plugin update` may `git reset --hard` it.

## Rules

- **Bump the version with every change on `main`** (semver), in
  `manifest.json`, `backend/Cargo.toml` and `backend/Cargo.lock` together.
  See CONTRIBUTING.md, "Versioning".
- **Every version tag gets a GitHub Release.** Right after pushing
  `vX.Y.Z`, run `gh release create vX.Y.Z --verify-tag --generate-notes
  --latest` and check that it shows on the Releases page. A tag without a
  release is an unfinished release.
- **A release is finished only when the marketplace has published it.**
  After tagging, open the `[Verify]: Omamail vX.Y.Z` issue in
  `omacom/omarchy-plugin-marketplace` and read every bot comment
  (validation, security baseline). Fix what they report here and ship the
  fix as a new patch release (bump, tag, `gh release create`): failed
  checks, and capabilities the baseline flags only because of wording
  (a README sentence saying the plugin needs no root was flagged as
  `privilege`). Capabilities that
  are real (`installer`, `service-management` for the user units) stay;
  explain them on the issue if a maintainer asks. Keep checking the issue
  until the update is published and the GitHub Release exists, and don't
  report the work as done before then. If only a maintainer's approval is
  left when you have to stop, say exactly that and link the issue.
- **Builds are `--locked`.** Never run a plain `cargo update`; add crates
  only when necessary, with the lockfile change in the same commit.
- **The app password stays in the keyring.** Never put it, mail text or
  verification codes in argv, logs or files (codes go to the 0600 state
  file, `wl-copy` via stdin, notifications over D-Bus directly). The one
  other file is the 0600 header cache (`listcache.rs`): subjects and
  senders, never bodies, deleted on sign-out.
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
- **Review every change before committing:** run `/code-review` and
  `/security-review` on the diff (other agents: an equivalent independent
  review), fix each real finding or say why not, and list what both
  reported in the PR body. Rerun them after fixing.
- Commits: Conventional Commits, no AI attribution or `Co-Authored-By`.
