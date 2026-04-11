#!/bin/bash
# test_mychat_fifo_broadcast.sh
# Tests: host correctly broadcasts messages to all clients in FIFO mode.
# Starts three clients; verifies cross-delivery of each client's message.
# Usage: test_mychat_fifo_broadcast.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

TEST_DIR="$(mktemp -d)"
HOST_PID=0
ALICE_PID=0
BOB_PID=0
CHARLIE_PID=0

cleanup() {
    [ "$HOST_PID"    -gt 0 ] && kill "$HOST_PID"    2>/dev/null || true
    [ "$ALICE_PID"   -gt 0 ] && kill "$ALICE_PID"   2>/dev/null || true
    [ "$BOB_PID"     -gt 0 ] && kill "$BOB_PID"     2>/dev/null || true
    [ "$CHARLIE_PID" -gt 0 ] && kill "$CHARLIE_PID" 2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    exec 4>&- 2>/dev/null || true
    exec 5>&- 2>/dev/null || true
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

# ── Start three clients ──────────────────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
mkfifo "$TEST_DIR/bob_in.fifo"
mkfifo "$TEST_DIR/charlie_in.fifo"

"$MYCHAT" -C "$HOST_FIFO" -m fifo -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"

"$MYCHAT" -C "$HOST_FIFO" -m fifo -n bob \
    < "$TEST_DIR/bob_in.fifo" > /dev/null 2> "$TEST_DIR/bob.log" &
BOB_PID=$!
exec 4>"$TEST_DIR/bob_in.fifo"

"$MYCHAT" -C "$HOST_FIFO" -m fifo -n charlie \
    < "$TEST_DIR/charlie_in.fifo" > /dev/null 2> "$TEST_DIR/charlie.log" &
CHARLIE_PID=$!
exec 5>"$TEST_DIR/charlie_in.fifo"

# Wait for all three to join
wait_for_count "$TEST_DIR/host.log" "Joined:" 3 60 \
    || fail "Not all clients joined within timeout"

# ── Send messages ────────────────────────────────────────────────────────────
echo "from_alice" >&3
echo "from_bob"   >&4

# Verify cross-delivery: bob and charlie must receive alice's message
wait_for_line "$TEST_DIR/bob.log"     "from_alice" 50 \
    || fail "Bob did not receive alice's message"
wait_for_line "$TEST_DIR/charlie.log" "from_alice" 50 \
    || fail "Charlie did not receive alice's message"

# Verify cross-delivery: alice and charlie must receive bob's message
wait_for_line "$TEST_DIR/alice.log"   "from_bob" 50 \
    || fail "Alice did not receive bob's message"
wait_for_line "$TEST_DIR/charlie.log" "from_bob" 50 \
    || fail "Charlie did not receive bob's message"

# ── Graceful shutdown ────────────────────────────────────────────────────────
exec 3>&-
exec 4>&-
exec 5>&-
wait_for_count "$TEST_DIR/host.log" "Left:" 3 60 \
    || fail "Not all clients sent LEAVE within timeout"

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
