#!/usr/bin/env bash
# Window-open and overlay latency in headless sway (nothing shows on the desktop).
#
#   scripts/bench-sway.sh <laqta binary> <label> [WxH]
#   python3 scripts/bench-analyze.py target/bench/run-<label> [target/bench/run-<other>]
#
# Needs sway, wtype and gnome-keyring-daemon (an unlocked keyring, so nothing prompts).
# Each run uses a scratch HOME and runtime dir under $LAQTA_BENCH_DIR (default target/bench):
# a cold start, 12 captures to fill history, each window opened 3 times (closed in between),
# settings and history with the main window open, and 3 region overlays (Escape cancels).
# Timings come from the `laqta::perf` trace points.
set -u
BIN=$(realpath "$1"); LABEL=$2; MODE=${3:-3840x2160}
BASE=$(realpath -m "${LAQTA_BENCH_DIR:-target/bench}")
OUT=$BASE/run-$LABEL; rm -rf "$OUT"; mkdir -p "$OUT/home"
RT=/tmp/lqs-$LABEL; rm -rf "$RT"; mkdir -m 700 -p "$RT"
export BIN OUT MODE
export HOME=$OUT/home XDG_CONFIG_HOME=$OUT/home/.config XDG_DATA_HOME=$OUT/home/.local/share XDG_RUNTIME_DIR=$RT
export RUST_LOG=info,laqta::perf=debug
unset WAYLAND_DISPLAY DISPLAY

cat > "$OUT/sway.conf" <<EOF
output * bg #336699 solid_color
output HEADLESS-1 resolution $MODE
EOF

cat > "$OUT/session.sh" <<'EOS'
#!/usr/bin/env bash
set -u
echo -n bench | gnome-keyring-daemon --replace --unlock --components=secrets -d >/dev/null 2>&1
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway -c "$OUT/sway.conf" > "$OUT/sway.log" 2>&1 &
SWAY=$!
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break; sleep 0.1; done
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.* 2>/dev/null | head -1)
close_all() { swaymsg '[app_id=".*"] kill' >/dev/null 2>&1; sleep 1.5; }
mark() { echo "$(date -u +%Y-%m-%dT%H:%M:%S.%6NZ) $*" >> "$OUT/marks"; }

mark cold
"$BIN" > "$OUT/stdout.log" 2>&1 &
sleep 6; close_all
mark seed
for _ in $(seq 12); do "$BIN" --action print-screen; sleep 0.8; done
sleep 3; close_all
for a in open-main-window open-settings open-history; do
  for i in 1 2 3; do mark "$a $i"; "$BIN" --action "$a" & sleep 3; close_all; done
done
"$BIN" --action open-main-window & sleep 3
for a in open-settings open-history; do mark "$a +main"; "$BIN" --action "$a" & sleep 3; done
close_all
for i in 1 2 3; do mark "region $i"; "$BIN" --action rectangle-region & sleep 3; wtype -k Escape; sleep 1.5; done
"$BIN" --action exit; sleep 2
kill $SWAY 2>/dev/null; wait $SWAY 2>/dev/null
EOS
chmod +x "$OUT/session.sh"
dbus-run-session -- "$OUT/session.sh" > "$OUT/dbus.log" 2>&1
rm -rf "$RT"
echo "$OUT"
