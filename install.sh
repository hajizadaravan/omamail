#!/bin/bash
# Builds the backend and installs Omamail into the Omarchy shell:
#   plugin    ~/.config/omarchy/plugins/reavann.omamail -> ./plugin (symlink)
#   launcher  ~/.local/share/applications/omamail.desktop
#   service   ~/.config/systemd/user/omamail-watch.service (new-mail notifications)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
plugin_link="$HOME/.config/omarchy/plugins/reavann.omamail"

cargo build --release --manifest-path "$root/backend/Cargo.toml"
install -Dm755 "$root/backend/target/release/omamail" "$root/plugin/bin/omamail"

if [[ -e $plugin_link && ! -L $plugin_link ]]; then
  echo "$plugin_link exists and is not a symlink; leaving it alone" >&2
  exit 1
fi
ln -sfn "$root/plugin" "$plugin_link"

install -Dm644 "$root/share/omamail.desktop" "$HOME/.local/share/applications/omamail.desktop"
install -Dm644 "$root/share/omamail.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/omamail.svg"
install -Dm644 "$root/share/omamail-watch.service" "$HOME/.config/systemd/user/omamail-watch.service"
systemctl --user daemon-reload
systemctl --user enable omamail-watch.service >/dev/null
# Starts now if an account is already signed in; otherwise signing in starts it.
systemctl --user restart omamail-watch.service || true

omarchy-shell shell rescanPlugins >/dev/null || true
sleep 1
omarchy plugin enable reavann.omamail

echo "Omamail installed. Open it with: omarchy-shell shell toggle reavann.omamail '{}'"
