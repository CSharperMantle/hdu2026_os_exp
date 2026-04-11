#!/bin/bash
# test_mychat_mq_join_leave.sh
# Tests: basic join/message/leave cycle and SIGINT host cleanup for MQ mode.
# Verifies /dev/mqueue/mychat-host is removed after host exits.
# Usage: test_mychat_mq_join_leave.sh path/to/mychat
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
    # Best-effort MQ cleanup in case host did not remove it
    rm -f /dev/mqueue/mychat-host 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ──────────────────────────────────────────────────────────────
"$MYCHAT" -H -m mq > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!

wait_for_line "$TEST_DIR/host.log" "Host MQ:" \
    || fail_harness "Host did not become ready"
[ -e /dev/mqueue/mychat-host ] \
    || fail_harness "Host MQ not created at /dev/mqueue/mychat-host"

# ── Start alice ──────────────────────────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C /mychat-host -m mq -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" \
    || fail "Alice did not join within timeout"

# ── Send a message ───────────────────────────────────────────────────────────
echo "ping_mq" >&3
wait_for_line "$TEST_DIR/host.log" "ping_mq" \
    || fail "Host did not receive alice's message"

# ── Graceful client shutdown ─────────────────────────────────────────────────
exec 3>&-
wait_for_line "$TEST_DIR/host.log" "Left:" \
    || fail "Host did not log alice's departure"

wait "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

# ── Graceful host shutdown ───────────────────────────────────────────────────
kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

# ── Verify MQ cleaned up ─────────────────────────────────────────────────────
if [ -e /dev/mqueue/mychat-host ]; then
    fail "Host MQ not removed after host exit: /dev/mqueue/mychat-host"
fi

echo "PASS"
