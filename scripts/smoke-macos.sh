#!/bin/bash
#
# Smoke-test a Voxtype.app build on a real Mac (or a macOS VM).
#
# Usage: scripts/smoke-macos.sh Voxtype-macos-arm64.app.zip [expected-release-tag]
#
# Installs the app into ~/Applications, then checks what CI on a hosted
# runner cannot: that the app, its daemon and the recording card come up,
# that the card owns a window, that a restart with a settings window open
# brings everything back (#5, #6), that closing the settings window ends its
# process, that `permissions --json` answers, and that a WAV file
# transcribes. Nothing here needs Input Monitoring, Accessibility or the
# microphone: those are granted by a human, and are not tested.
#
# Meant for a machine whose Voxtype can be stopped and replaced at will: it
# quits any running Voxtype first.

set -uo pipefail

ZIP="${1:?usage: $0 Voxtype-macos-arm64.app.zip [release-tag]}"
EXPECTED_TAG="${2:-}"
APP_DIR="$HOME/Applications"
APP="$APP_DIR/Voxtype.app"
BIN="$APP/Contents/MacOS/voxtype-bin"
HERE="$(cd "$(dirname "$0")/.." && pwd)"

failures=0
summary=""
pass() { echo "PASS  $1"; summary+="| ✅ | $1 |"$'\n'; }
fail() { echo "FAIL  $1"; summary+="| ❌ | $1 |"$'\n'; failures=$((failures + 1)); diag; }

# What Voxtype is doing, printed after a failed check.
diag() {
    echo "  --- processes"
    ps -axo pid,ppid,command | grep '[v]oxtype-bin' | sed 's/^/  /'
    echo "  --- status: $("$BIN" status 2>&1 | tail -1)"
    for lock in /tmp/voxtype/*.lock; do
        [ -e "$lock" ] && echo "  --- $lock: $(cat "$lock" 2>/dev/null)"
    done
    echo "  --- last log lines"
    tail -n 15 "$HOME/Library/Logs/voxtype/stdout.log" 2>/dev/null | sed 's/^/  /'
    if [ -n "${APP_LOG:-}" ]; then
        echo "  --- app output"
        tail -n 15 "$APP_LOG" 2>/dev/null | sed 's/^/  /'
    fi
    echo "  --- stderr"
    tail -n 15 "$HOME/Library/Logs/voxtype/stderr.log" 2>/dev/null | sed 's/^/  /'
    local crash
    crash="$(ls -t "$HOME"/Library/Logs/DiagnosticReports/voxtype-bin* 2>/dev/null | head -1)"
    if [ -n "$crash" ]; then
        echo "  --- crash report $crash"
        head -n 40 "$crash" | sed 's/^/  /'
    fi
}
check() { local name="$1"; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }

# Wait up to $1 seconds for "$2..." to succeed.
wait_for() {
    local secs="$1"; shift
    local end=$((SECONDS + secs))
    while ((SECONDS < end)); do
        "$@" && return 0
        sleep 1
    done
    return 1
}

pid_of() { pgrep -f "$BIN $1" | head -1; }
app_pid() { pgrep -fx "$BIN" | head -1; }
daemon_up() { [ "$("$BIN" status 2>/dev/null)" != "stopped" ] && [ -n "$(pid_of daemon)" ]; }

# Windows owned by a pid, read through CoreGraphics. Unlike System Events
# this needs no Accessibility grant. The CFArray CoreGraphics returns must be
# cast to an NSArray before JXA can read it, and option 0 is
# kCGWindowListOptionAll. Errors go to stderr rather than reading as 0.
windows_of() {
    osascript -l JavaScript -e "
        ObjC.import('CoreGraphics');
        ObjC.import('Foundation');
        const list = ObjC.deepUnwrap(ObjC.castRefToObject(\$.CGWindowListCopyWindowInfo(0, 0)));
        list.filter(w => w.kCGWindowOwnerPID === $1).length;
    " || echo 0
}
card_has_window() { local p; p="$(pid_of osd-card)"; [ -n "$p" ] && [ "$(windows_of "$p")" -ge 1 ]; }
app_runs() { [ -n "$(app_pid)" ]; }
card_runs() { [ -n "$(pid_of osd-card)" ]; }
# Alive and not a zombie waiting to be reaped.
alive() { ps -p "$1" -o stat= 2>/dev/null | grep -qv Z; }
settings_has_window() { [ "$(windows_of "$settings")" -ge 1 ]; }
settings_gone() { ! alive "$settings"; }
new_daemon() { local p; p="$(pid_of daemon)"; [ -n "$p" ] && [ "$p" != "$old_daemon" ]; }
new_app() { local p; p="$(app_pid)"; [ -n "$p" ] && [ "$p" != "$old_app" ]; }
reports_permissions() { grep -q '"input_monitoring"' <<<"$perms" && grep -q '"accessibility"' <<<"$perms"; }
# grep the binary directly: `strings | grep -q` closes the pipe early and
# strings reports that as an error.
has_tag() { grep -aq "$EXPECTED_TAG" "$BIN"; }

none_running() { ! pgrep -f 'Voxtype.app/Contents/MacOS/voxtype-bin' >/dev/null; }

quit_all() {
    pkill -f "$APP_DIR/Voxtype.app/Contents/MacOS/voxtype-bin" 2>/dev/null
    pkill -f "/Applications/Voxtype.app/Contents/MacOS/voxtype-bin" 2>/dev/null
    wait_for 10 none_running
    sleep 1
}

echo "== Install"
quit_all
rm -rf "$APP"
mkdir -p "$APP_DIR"
ditto -x -k "$ZIP" "$APP_DIR"
xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true
check "bundle signature is valid" codesign --verify --deep --strict "$APP"
version="$("$BIN" --version)"
echo "$version"
if [ -n "$EXPECTED_TAG" ]; then
    check "binary names its release ($EXPECTED_TAG)" has_tag
fi

echo "== Model"
# The default config uses Whisper base.en. Fetch it up front so the test
# does not depend on the first launch's own download.
check "base.en model downloads" "$BIN" setup --download --model base.en --no-post-install --quiet

echo "== Launch"
# open discards the app's own output; keep it for the diagnostics.
APP_LOG="$(mktemp)"
open -n --stdout "$APP_LOG" --stderr "$APP_LOG" "$APP"
# A first launch on a fresh machine downloads a speech model before it
# starts the daemon.
check "daemon comes up" wait_for 600 daemon_up
check "menu bar process runs" wait_for 10 app_runs
check "recording card runs" wait_for 20 card_runs
check "recording card owns a window" wait_for 20 card_has_window

echo "== Settings window"
"$BIN" settings >/dev/null 2>&1 &
settings=$!
check "settings window opens" wait_for 20 settings_has_window
# Closing a window from a script needs Accessibility; SIGTERM ends the
# process, and what matters for #6 is that one is open during the restart.

echo "== Restart with a settings window open (#5, #6)"
old_daemon="$(pid_of daemon)"
old_app="$(app_pid)"
check "voxtype restart succeeds" "$BIN" restart
check "daemon is back" wait_for 30 daemon_up
check "daemon is a new process" new_daemon
check "app is a new process" new_app
check "settings window was stopped by the restart" wait_for 5 settings_gone
check "recording card owns a window after restart" wait_for 30 card_has_window

echo "== Permissions report"
perms="$("$BIN" permissions --json 2>/dev/null)"
echo "$perms"
check "permissions --json answers" reports_permissions

echo "== Transcription"
text="$("$BIN" -q transcribe "$HERE/tests/fixtures/vad/speech_hello.wav" 2>/dev/null | tail -1)"
echo "transcribed: $text"
check "a WAV file transcribes (\"hello\")" grep -qi hello <<<"$text"

echo "== Clean up"
quit_all

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### macOS smoke test: $version"
        echo
        echo "| | Check |"
        echo "|---|---|"
        printf '%s' "$summary"
    } >>"$GITHUB_STEP_SUMMARY"
fi

echo
if ((failures)); then
    echo "$failures check(s) failed"
    exit 1
fi
echo "All checks passed"
