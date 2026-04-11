#!/bin/bash
# test_mychat_fifo_join_leave.sh
# Tests: basic join/message/leave cycle and SIGINT host cleanup for FIFO mode.
# Usage: test_mychat_fifo_join_leave.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

TEST_DIR="$(mktemp -d)"
HOST_PID=0
ALICE_PID=0

cleanup() {
    [ "$HOST_PID"  -gt 0 ] && kill "$HOST_PID"  2>/dev/null || true
    [ "$ALICE_PID" -gt 0 ] && kill "$ALICE_PID" 2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ──────────────────────────────────────────────────────────────
"$MYCHAT" -H -m fifo > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!

wait_for_line "$TEST_DIR/host.log" "Control FIFO:" \
    || fail_harness "Host did not become ready"

HOST_FIFO="$(grep "Control FIFO:" "$TEST_DIR/host.log" \
    | tail -1 | sed 's/.*Control FIFO: //')"
[ -n "$HOST_FIFO" ] || fail_harness "Could not extract FIFO path"
[ -e "$HOST_FIFO" ] || fail_harness "FIFO does not exist: $HOST_FIFO"

# ── Start alice with a stdin pipe ───────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"   # keep write end open

wait_for_line "$TEST_DIR/host.log" "Joined:" \
    || fail "Alice did not join within timeout"

# ── Send a message ───────────────────────────────────────────────────────────
echo "ping_fifo" >&3
wait_for_line "$TEST_DIR/host.log" "ping_fifo" \
    || fail "Host did not receive alice's message"

# ── Graceful client shutdown (EOF → LEAVE) ──────────────────────────────────
exec 3>&-
wait_for_line "$TEST_DIR/host.log" "Left:" \
    || fail "Host did not log alice's departure"

wait "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

# ── Graceful host shutdown (SIGINT) ─────────────────────────────────────────
kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

# ── Verify FIFO is cleaned up ───────────────────────────────────────────────
if [ -e "$HOST_FIFO" ]; then
    fail "Control FIFO not deleted after host exit: $HOST_FIFO"
fi

echo "PASS"
