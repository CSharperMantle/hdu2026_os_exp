#!/bin/bash
# test_mychat_shmem_stress_concurrent_send.sh
# Tests: SHMEM semaphore mutual exclusion under concurrent writers.
# space_sem serializes writers: clients block until the host consumes each slot.
# Three clients send 10 msgs each simultaneously; all 30 must arrive intact.
# Usage: test_mychat_shmem_stress_concurrent_send.sh path/to/mychat
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
    rm -f /dev/shm/mychat-host 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ──────────────────────────────────────────────────────────────
"$MYCHAT" -H -m shmem > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!

wait_for_line "$TEST_DIR/host.log" "Host SHM:" \
    || fail_harness "Host did not become ready"

# ── Start three clients ──────────────────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
mkfifo "$TEST_DIR/bob_in.fifo"
mkfifo "$TEST_DIR/charlie_in.fifo"

"$MYCHAT" -C /mychat-host -m shmem -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"

"$MYCHAT" -C /mychat-host -m shmem -n bob \
    < "$TEST_DIR/bob_in.fifo" > /dev/null 2> "$TEST_DIR/bob.log" &
BOB_PID=$!
exec 4>"$TEST_DIR/bob_in.fifo"

"$MYCHAT" -C /mychat-host -m shmem -n charlie \
    < "$TEST_DIR/charlie_in.fifo" > /dev/null 2> "$TEST_DIR/charlie.log" &
CHARLIE_PID=$!
exec 5>"$TEST_DIR/charlie_in.fifo"

wait_for_count "$TEST_DIR/host.log" "Joined:" 3 60 \
    || fail_harness "Not all clients joined"

# ── Fire all senders concurrently ────────────────────────────────────────────
# space_sem forces serialization: only one client writes at a time.
{
    for i in {0..9}; do echo "STRESS_alice_$i"; done
} >&3 &
{
    for i in {0..9}; do echo "STRESS_bob_$i"; done
} >&4 &
{
    for i in {0..9}; do echo "STRESS_charlie_$i"; done
} >&5 &
wait

exec 3>&-
exec 4>&-
exec 5>&-

# ── All messages must arrive (allow 15 s for serialized delivery) ─────────────
wait_for_count "$TEST_DIR/host.log" "STRESS_" "$TOTAL" 150 || {
    actual="$(grep -c "STRESS_" "$TEST_DIR/host.log" 2>/dev/null || echo 0)"
    fail "Expected $TOTAL STRESS messages at host, got $actual"
}

if grep -q "Cannot handle frame" "$TEST_DIR/host.log" 2>/dev/null; then
    fail "Host reported frame parsing errors"
fi

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
