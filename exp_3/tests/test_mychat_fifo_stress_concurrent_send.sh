#!/bin/bash
# test_mychat_fifo_stress_concurrent_send.sh
# Tests: FIFO atomicity under concurrent writers.
# Three clients flood the control FIFO simultaneously (10 msgs each = 30 total).
# All frames must arrive intact; no "Cannot handle frame" warnings expected.
# Usage: test_mychat_fifo_stress_concurrent_send.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

N_MSGS=10
N_CLIENTS=3
TOTAL=$((N_MSGS * N_CLIENTS))

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

wait_for_count "$TEST_DIR/host.log" "Joined:" 3 60 \
    || fail_harness "Not all clients joined"

# ── Fire all senders concurrently ────────────────────────────────────────────
{
    for i in {0..9}; do echo "STRESS_alice_$i"; done
} >&3 &
{
    for i in {0..9}; do echo "STRESS_bob_$i"; done
} >&4 &
{
    for i in {0..9}; do echo "STRESS_charlie_$i"; done
} >&5 &
wait   # wait for all feeder subshells to finish writing

# ── Close stdin pipes → graceful client exit ─────────────────────────────────
exec 3>&-
exec 4>&-
exec 5>&-

# ── Assert all messages arrived at host ──────────────────────────────────────
# Each STRESS_* tag is unique; host logs one line per received MSG frame.
wait_for_count "$TEST_DIR/host.log" "STRESS_" "$TOTAL" 150 || {
    actual="$(grep -c "STRESS_" "$TEST_DIR/host.log" 2>/dev/null || echo 0)"
    fail "Expected $TOTAL STRESS messages at host, got $actual"
}

# ── No frame corruption ───────────────────────────────────────────────────────
if grep -q "Cannot handle frame" "$TEST_DIR/host.log" 2>/dev/null; then
    fail "Host reported frame parsing errors (frame corruption)"
fi

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
