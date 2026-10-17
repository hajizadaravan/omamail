#!/bin/bash
# Builds the Omamail backend and installs what `omarchy plugin add` cannot:
#   backend   ~/.local/lib/omamail/omamail, built from ./backend. It lives
#             outside the git checkout, so `omarchy plugin update` can never
#             swap in a binary; only a rebuild you run changes it.
#   launcher  ~/.local/share/applications/omamail.desktop + icon
#   service   ~/.config/systemd/user/omamail-watch.service (new-mail notifications)
#
# Run it from the plugin folder after `omarchy plugin add`, and again after
# `omarchy plugin update reavann.omamail`. Run from a checkout elsewhere, it
# also copies the plugin files there (development). A copy, not a symlink:
# `omarchy plugin update` follows links into git checkouts and can
# `git reset --hard` them, which would take uncommitted work with it.
set -euo pipefail

root="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
plugins_dir="$HOME/.config/omarchy/plugins"
plugin_dir="$plugins_dir/reavann.omamail"

if ! command -v cargo >/dev/null; then
  echo "Rust is needed to build the backend. Install it with: omarchy install dev-env rust" >&2
  exit 1
fi
# --locked: an update that changes Cargo.toml without the reviewed lockfile
# fails here instead of fetching (and running build scripts of) new crates.
# Build output stays out of the plugin folder: the shell watches that tree
# and would reload plugins for every file cargo writes.
export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/omamail/target"
cargo build --release --locked --manifest-path "$root/backend/Cargo.toml"
install -Dm755 "$CARGO_TARGET_DIR/release/omamail" "$HOME/.local/lib/omamail/omamail"

if [[ -L $plugin_dir || $root != "$(realpath -m "$plugin_dir")" ]]; then
  if [[ -d $plugin_dir/.git && ! -L $plugin_dir ]]; then
    echo "$plugin_dir is a plugin installed with 'omarchy plugin add'; leaving it alone" >&2
    exit 1
  fi
  mkdir -p "$plugins_dir"
  [[ -L $plugin_dir ]] && rm "$plugin_dir"
  rsync -a --delete --exclude .git --exclude /backend/target/ "$root/" "$plugin_dir/"
fi

install -Dm644 "$root/share/omamail.desktop" "$HOME/.local/share/applications/omamail.desktop"
install -Dm644 "$root/share/omamail.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/omamail.svg"
install -Dm644 "$root/share/omamail-watch.service" "$HOME/.config/systemd/user/omamail-watch.service"
if systemctl --user daemon-reload 2>/dev/null; then
  systemctl --user enable omamail-watch.service >/dev/null
  # Starts now if an account is already signed in; otherwise signing in starts it.
  systemctl --user restart omamail-watch.service || true
else
  echo "No user session bus: run 'systemctl --user enable --now omamail-watch.service' from your desktop session." >&2
fi

omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
sleep 1
if ! omarchy plugin enable reavann.omamail >/dev/null; then
  echo "Could not enable the plugin yet; run: omarchy plugin enable reavann.omamail" >&2
fi

if ! command -v zenity >/dev/null; then
  echo "Note: attaching files needs zenity: omarchy pkg add zenity" >&2
fi

echo "Omamail installed. Open it from the app launcher, or with: omarchy-shell shell toggle reavann.omamail '{}'"
