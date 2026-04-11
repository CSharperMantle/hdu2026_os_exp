#!/bin/bash
# test_mychat_fifo_zombie.sh
# Tests: host detects and reaps a SIGKILL'd client (no LEAVE frame) via
# open(O_WRONLY|O_NONBLOCK) on the dead client's data FIFO.
# After reaping, verifies the host is still operational.
# Usage: test_mychat_fifo_zombie.sh path/to/mychat
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

# ── Start zombie client ──────────────────────────────────────────────────────
mkfifo "$TEST_DIR/zombie_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n zombie \
    < "$TEST_DIR/zombie_in.fifo" > /dev/null 2> "$TEST_DIR/zombie.log" &
ZOMBIE_PID=$!
exec 3>"$TEST_DIR/zombie_in.fifo"   # keep write end open so client stays up

wait_for_line "$TEST_DIR/host.log" "Joined:" \
    || fail_harness "Zombie client did not join"

# ── Kill zombie without LEAVE ────────────────────────────────────────────────
# SIGKILL: no defers, no LEAVE frame. Data FIFO read end closed by kernel.
kill -KILL "$ZOMBIE_PID" 2>/dev/null || true
ZOMBIE_PID=0
exec 3>&-   # close stdin pipe; now FIFO has no readers → probe will fail

# ── Expect zombie reaping (FIFO probe interval ≈ 500 ms) ────────────────────
# Use 80 iterations = 8 s to cover variability.
wait_for_line "$TEST_DIR/host.log" "Reaped zombie" 80 \
    || fail "Host did not reap zombie client within timeout"

# ── Verify host is still operational ─────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 4>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" 50 \
    || fail "Host rejected new client after zombie reap"

exec 4>&-
wait "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
