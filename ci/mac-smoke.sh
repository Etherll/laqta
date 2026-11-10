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
  "$bin" "$@" >>"$out/steps.log" 2>&1
  echo "   exit $?" >>"$out/steps.log"
  sleep "$wait"
  shot "$name"
  alive "$name"
}

alive() {
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "Laqta died after: $1" | tee -a "$out/steps.log"
    exit 1
  fi
}

escape() {
  osascript -e 'tell application "System Events" to key code 53' >>"$out/steps.log" 2>&1
}

# An action that waits for a selection: its request stays open until the selection ends. The
# overlay is photographed, then Escape ends it; a request still open 20 s later is a hang, and
# both processes are sampled so the cause can be read without a Mac.
pick() { # pick <name> <args...>
  name=$1
  shift
  echo "== $name: $* (then Escape)" | tee -a "$out/steps.log"
  "$bin" "$@" >>"$out/steps.log" 2>&1 &
  cli=$!
  sleep 5
  shot "$name"
  escape
  waited=0
  while kill -0 "$cli" 2>/dev/null && [ "$waited" -lt 20 ]; do
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 "$cli" 2>/dev/null; then
    echo "   no reply 20 s after Escape: $name" | tee -a "$out/steps.log"
    sample "$pid" 3 -file "$out/hang-$name-app.txt" >/dev/null 2>&1
    sample "$cli" 1 -file "$out/hang-$name-cli.txt" >/dev/null 2>&1
    kill "$cli" 2>/dev/null
  else
    wait "$cli"
    echo "   exit $?" >>"$out/steps.log"
  fi
  shot "$name-after-escape"
  alive "$name"
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
pick region-overlay --action rectangle-region
pick record-region --action start-screen-recorder
pick ocr --action ocr
"$bin" --action stop-screen-recording >>"$out/steps.log" 2>&1 &
sleep 3
kill $! 2>/dev/null

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
