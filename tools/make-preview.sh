#!/bin/bash
# Regenerates preview.png for the plugin listing: builds the backend with the
# `demo` feature (a made-up mailbox, see backend/src/demo.rs), shoots the
# Omamail window in three themes on an empty workspace, and joins the shots
# into diagonal slices with the verification-code toast in the corner.
#
# It switches your theme and workspace while it runs (about a minute) and
# then restores them, the real backend binary and the watcher, even on
# failure. Run it from an Omarchy session with Omamail installed, on a
# 1920x1080 monitor at scale 1; needs grim and ImageMagick. Look at
# preview.png before committing it: another app's notification arriving
# during a shot would be in it.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
bin="$HOME/.local/lib/omamail/omamail"
state="$HOME/.local/state/omarchy/current"
themes=("Tokyo Night" "Catppuccin Latte" "Gruvbox")
slugs=(tokyo-night catppuccin-latte gruvbox)
shot_ws=9
font=$(fc-list ':family=JetBrainsMono Nerd Font:style=Bold' file | head -1 | cut -d: -f1)
font=${font:-DejaVu-Sans-Bold}

ws_info() { hyprctl activeworkspace -j | python3 -c 'import json,sys;w=json.load(sys.stdin);print(w["id"],w["windows"],w["monitor"])'; }
focus_ws() { hyprctl dispatch "hl.dsp.focus({ workspace = \"$1\" })" >/dev/null; }
notify() {
  gdbus call --session --dest org.freedesktop.Notifications --object-path /org/freedesktop/Notifications "$@"
}

[[ -x $bin ]] || { echo "Omamail is not installed ($bin); run ./install.sh first" >&2; exit 1; }
busy=$(hyprctl workspaces -j | python3 -c "import json,sys;print(sum(w['windows'] for w in json.load(sys.stdin) if w['id']==$shot_ws))")
[[ $busy == 0 ]] || { echo "workspace $shot_ws has windows; empty it first" >&2; exit 1; }
# The crops below are in pixels of a 1920x1080 screen.
size=$(hyprctl monitors -j | python3 -c 'import json,sys;m=[m for m in json.load(sys.stdin) if m["focused"]][0];print(m["width"],m["height"],m["scale"])')
[[ $size == "1920 1080 1.0" || $size == "1920 1080 1" ]] || { echo "needs a 1920x1080 monitor at scale 1 (focused one is $size)" >&2; exit 1; }

echo "building the demo backend…"
CARGO_TARGET_DIR="$work/target" cargo build --release --locked --features demo \
  --manifest-path "$root/backend/Cargo.toml" >"$work/build.log" 2>&1 || { cat "$work/build.log" >&2; exit 1; }

read -r orig_ws _ _ < <(ws_info)
orig_theme=$(cat "$state/theme.name")
orig_bg=$(readlink -f "$state/background")
watcher=$(systemctl --user is-active omamail-watch || true)
cp "$bin" "$work/omamail.real"
id=""

restore() {
  [[ -n $id ]] && notify --method org.freedesktop.Notifications.CloseNotification "$id" >/dev/null 2>&1
  omarchy-shell shell hide reavann.omamail >/dev/null 2>&1 || true
  # `install` replaces the file instead of writing into it, so a demo
  # `serve` still running from it cannot make this fail (ETXTBSY).
  local restored=1
  install -m755 "$work/omamail.real" "$bin" || restored=0
  omarchy theme set "$orig_theme" >/dev/null 2>&1 || true
  sleep 2
  # The full path: one inside the current theme folder exists again now that
  # the theme is back, and one anywhere else never moved.
  omarchy theme bg set "$orig_bg" >/dev/null 2>&1 || true
  focus_ws "$orig_ws" || true
  if ((restored)); then
    [[ $watcher == active ]] && systemctl --user start omamail-watch
    rm -rf "$work"
    echo "restored theme $orig_theme, workspace $orig_ws"
  else
    echo "could not put the real backend back; it is at $work/omamail.real:" >&2
    echo "  install -m755 $work/omamail.real $bin && systemctl --user start omamail-watch" >&2
  fi
}
trap restore EXIT

# Real notifications must not land in the shots.
systemctl --user stop omamail-watch
omarchy-shell shell hide reavann.omamail >/dev/null 2>&1 || true
install -m755 "$work/target/release/omamail" "$bin"
focus_ws "$shot_ws"

for i in "${!themes[@]}"; do
  omarchy theme set "${themes[$i]}" >/dev/null 2>&1
  sleep 4
  omarchy-shell shell summon reavann.omamail '{"folder":"INBOX","uid":211}' >/dev/null
  sleep 3
  id=$(notify --method org.freedesktop.Notifications.Notify "Omamail" 0 "" "Lumen" \
    "<b>482913</b>  ·  click to copy
lumen.example  ·  Your Lumen sign-in code" "[]" "{'omarchy-glyph': <'$(printf '\U000f01ee')'>}" 0 \
    | sed -E 's/^\(uint32 ([0-9]+),\)$/\1/')
  sleep 1.5
  # Only the Omamail window may be on screen: never capture anything else.
  read -r ws windows monitor < <(ws_info)
  if [[ $ws != "$shot_ws" || $windows != 1 ]]; then
    echo "workspace $ws has $windows windows, not just Omamail; stopping" >&2
    exit 1
  fi
  grim -o "$monitor" "$work/${slugs[$i]}.png"
  notify --method org.freedesktop.Notifications.CloseNotification "$id" >/dev/null
  id=""
  omarchy-shell shell hide reavann.omamail >/dev/null
  sleep 1
  echo "shot ${themes[$i]}"
done

# 1280x720 from the top left of each shot, pixel for pixel: sidebar, list
# and the start of the reader. Three diagonal slices, light seams with a
# soft shadow, the toast from the last theme, a label per slice.
for s in "${slugs[@]}"; do
  magick "$work/$s.png" -crop 1280x720+0+0 +repage "$work/$s-crop.png"
done
magick -size 1280x720 xc:black -fill white -draw "polygon 450,0 900,0 800,720 350,720" "$work/mask2.png"
magick -size 1280x720 xc:black -fill white -draw "polygon 900,0 1280,0 1280,720 800,720" "$work/mask3.png"
magick "$work/${slugs[0]}-crop.png" "$work/${slugs[1]}-crop.png" "$work/mask2.png" -composite "$work/out.png"
magick "$work/out.png" "$work/${slugs[2]}-crop.png" "$work/mask3.png" -composite "$work/out.png"
seams=(-draw "line 450,0 350,720" -draw "line 900,0 800,720")
magick -size 1280x720 xc:none -stroke 'rgba(0,0,0,0.45)' -strokewidth 10 "${seams[@]}" -blur 0x4 "$work/shadow.png"
magick "$work/out.png" "$work/shadow.png" -composite \
  -stroke 'rgba(255,255,255,0.9)' -strokewidth 3 "${seams[@]}" "$work/out.png"
magick "$work/${slugs[2]}.png" -crop 386x80+1532+28 +repage "$work/toast.png"
magick "$work/out.png" "$work/toast.png" -geometry +886+30 -composite "$work/out.png"

centers=(190 625 1050)
for i in "${!themes[@]}"; do
  magick -background none -fill white -font "$font" -pointsize 15 label:"${themes[$i]}" "$work/text.png"
  read -r w h < <(magick identify -format '%w %h\n' "$work/text.png")
  w=$((w + 28)) h=$((h + 14))
  magick -size "${w}x${h}" xc:none -fill 'rgba(10,10,14,0.78)' -draw "roundrectangle 0,0 $((w - 1)),$((h - 1)) 12,12" \
    "$work/text.png" -gravity center -composite "$work/pill.png"
  magick "$work/out.png" "$work/pill.png" -geometry "+$((centers[i] - w / 2))+668" -composite "$work/out.png"
done
magick "$work/out.png" -strip "$root/preview.png"
echo "wrote $root/preview.png"
