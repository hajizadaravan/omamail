#!/bin/bash
# Updates Omamail: `omarchy plugin update` shows the incoming diff and asks
# before anything changes; only an accepted update is rebuilt (install.sh)
# and loaded (shell restart). The "update available" notification opens
# this in a terminal with --pause, so the window stays open for the result.
#
# The braces make bash read the whole script before running it: the update
# below may replace this very file.
{
  set -uo pipefail
  plugin_dir="$HOME/.config/omarchy/plugins/reavann.omamail"

  finish() {
    [[ ${1:-} == --pause ]] && read -rp "Press Enter to close. " _
    exit "$2"
  }

  if [[ ! -d $plugin_dir/.git ]]; then
    echo "$plugin_dir is not a git checkout (installed from a checkout elsewhere?)." >&2
    echo "Update that checkout and run its install.sh instead." >&2
    finish "${1:-}" 1
  fi

  before="$(git -C "$plugin_dir" rev-parse HEAD)"
  omarchy plugin update reavann.omamail
  status=$?
  after="$(git -C "$plugin_dir" rev-parse HEAD)"
  if [[ $before == "$after" ]]; then
    if ((status != 0)); then
      echo "The update failed or was declined (see above); nothing was installed." >&2
      finish "${1:-}" 1
    fi
    echo "Nothing was updated."
    finish "${1:-}" 0
  fi

  if ((status != 0)); then
    echo "The update stopped part-way (see above); not building it. Run it again, or run $plugin_dir/install.sh once it is complete." >&2
    finish "${1:-}" 1
  fi
  if ! "$plugin_dir/install.sh"; then
    echo "The update was downloaded but the build failed; fix the error above and run $plugin_dir/install.sh." >&2
    finish "${1:-}" 1
  fi
  omarchy restart shell
  echo "Omamail is up to date."
  finish "${1:-}" 0
}
