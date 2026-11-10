#!/bin/sh
# Starts Laqta.app on a macOS runner, drives it through the CLI, and leaves screenshots and the
# log in <out>. This is how windows are reviewed without a Mac.
#   ci/mac-smoke.sh <Laqta.app> <out-dir>
# The script fails only when Laqta does not start or dies; what each step shows is read from the
# screenshots and the log.
set -u
app=$1 out=$2
bin="$app/Contents/MacOS/laqta"
mkdir -p "$out"
log="$out/laqta.log"
n=0

shot() {
  n=$((n + 1))
  name=$(printf '%02d-%s' "$n" "$1")
  screencapture -x "$out/$name.png" 2>>"$out/screencapture.log" || echo "screencapture failed for $name" >>"$out/steps.log"
}

step() { # step <name> <seconds> <args...>
  name=$1 wait=$2
  shift 2
  echo "== $name: $*" | tee -a "$out/steps.log"
  "$bin" "$@" >>"$out/steps.log" 2>&1 &
  cli=$!
  waited=0
  while kill -0 "$cli" 2>/dev/null && [ "$waited" -lt 30 ]; do
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 "$cli" 2>/dev/null; then
    # The request did not return. Keep both stacks, so the cause can be read without a Mac.
    echo "   no reply after 30 s: $name" | tee -a "$out/steps.log"
    sample "$pid" 3 -file "$out/hang-$name-app.txt" >/dev/null 2>&1
    sample "$cli" 1 -file "$out/hang-$name-cli.txt" >/dev/null 2>&1
    kill "$cli" 2>/dev/null
  else
    wait "$cli"
    echo "   exit $?" >>"$out/steps.log"
  fi
  sleep "$wait"
  shot "$name"
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "Laqta died after: $name" | tee -a "$out/steps.log"
    exit 1
  fi
}

sw_vers >"$out/system.txt" 2>&1
system_profiler SPDisplaysDataType >>"$out/system.txt" 2>&1
"$bin" --version | tee -a "$out/system.txt" || exit 1
codesign -dv --entitlements - "$app" >>"$out/system.txt" 2>&1

RUST_BACKTRACE=1 "$bin" -sandbox >"$log" 2>&1 &
pid=$!
sleep 10
if ! kill -0 "$pid" 2>/dev/null; then
  echo "Laqta did not stay running" | tee -a "$out/steps.log"
  cat "$log"
  exit 1
fi
shot start

step main-window 4 --action open-main-window
step settings 4 --action open-settings
step history 4 --action open-history
step color-picker 4 --action color-picker
step print-screen 6 --action print-screen
osascript -e 'clipboard info' >"$out/clipboard-after-print-screen.txt" 2>&1
step region-overlay 4 --action rectangle-region
osascript -e 'tell application "System Events" to key code 53' >>"$out/steps.log" 2>&1 # Esc
sleep 2
shot after-escape
step record-start 6 --action start-screen-recorder
step record-stop 6 --action stop-screen-recording
step ocr 5 --action ocr
osascript -e 'tell application "System Events" to key code 53' >>"$out/steps.log" 2>&1
sleep 1

ps -o rss=,cputime= -p "$pid" >"$out/process.txt" 2>&1
"$bin" --action exit >>"$out/steps.log" 2>&1 &
sleep 5
kill $! 2>/dev/null
if kill -0 "$pid" 2>/dev/null; then
  echo "Laqta did not exit on --action exit" | tee -a "$out/steps.log"
  kill "$pid"
fi
echo "---- log tail"
tail -n 120 "$log"
ls -la "$out"
