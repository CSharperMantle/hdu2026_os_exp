#!/bin/bash
# test_mychat_shmem_zombie.sh
# Tests: SHMEM zombie detection via delivery retry counter.
# The host increments n_retries each time csem_timedwait times out (500 ms).
# After MAX_RETRY_COUNT=3 failures the client is reaped.
# Trigger: SIGKILL the zombie, then have a live client send 3+ messages to
# force broadcast attempts (which time out waiting on the zombie's space_sem).
# Usage: test_mychat_shmem_zombie.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

TEST_DIR="$(mktemp -d)"
HOST_PID=0
ZOMBIE_PID=0
ALICE_PID=0

cleanup() {
    [ "$HOST_PID"   -gt 0 ] && kill "$HOST_PID"   2>/dev/null || true
    [ "$ZOMBIE_PID" -gt 0 ] && kill "$ZOMBIE_PID" 2>/dev/null || true
    [ "$ALICE_PID"  -gt 0 ] && kill "$ALICE_PID"  2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    exec 4>&- 2>/dev/null || true
    rm -f /dev/shm/mychat-host 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ──────────────────────────────────────────────────────────────
"$MYCHAT" -H -m shmem > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!

wait_for_line "$TEST_DIR/host.log" "Host SHM:" \
    || fail_harness "Host did not become ready"

# ── Start zombie client ──────────────────────────────────────────────────────
mkfifo "$TEST_DIR/zombie_in.fifo"
"$MYCHAT" -C /mychat-host -m shmem -n zombie \
    < "$TEST_DIR/zombie_in.fifo" > /dev/null 2> "$TEST_DIR/zombie.log" &
ZOMBIE_PID=$!
exec 3>"$TEST_DIR/zombie_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" \
    || fail_harness "Zombie client did not join"

# ── Start live client (alice) to trigger broadcast attempts ─────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C /mychat-host -m shmem -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 4>"$TEST_DIR/alice_in.fifo"

wait_for_count "$TEST_DIR/host.log" "Joined:" 2 30 \
    || fail_harness "Alice did not join"

# ── Kill zombie ──────────────────────────────────────────────────────────────
kill -KILL "$ZOMBIE_PID" 2>/dev/null || true
ZOMBIE_PID=0
exec 3>&-
# Zombie's space_sem is now stuck at 0 (no one posts it).
# Each broadcast to zombie will time out after 500 ms and increment n_retries.

# ── Trigger broadcasts: alice sends 3 messages ───────────────────────────────
# Each triggers one broadcast round; zombie delivery times out each time.
# Allow generous pauses so host can process in between.
echo "trigger_1" >&4
sleep 0.8
echo "trigger_2" >&4
sleep 0.8
echo "trigger_3" >&4
sleep 0.8

# ── Wait for zombie reap (3 × 500 ms = 1.5 s minimum; allow 10 s) ───────────
wait_for_line "$TEST_DIR/host.log" "Reaped zombie" 100 \
    || fail "Host did not reap zombie client within timeout"

# ── Shutdown ─────────────────────────────────────────────────────────────────
exec 4>&-
wait "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
