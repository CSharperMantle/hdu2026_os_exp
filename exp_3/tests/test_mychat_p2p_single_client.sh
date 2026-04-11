#!/bin/bash
# test_mychat_p2p_single_client.sh
# Tests: P2P enforces single-client exclusivity; second client blocks until first
# disconnects, then successfully joins.
#
# NOTE: p2pRecv calls csem.wait with @panic on error, so SIGINT cannot be used
# for clean teardown. Processes are force-killed and the SHM is removed manually.
# Usage: test_mychat_p2p_single_client.sh path/to/mychat
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
P2P_SHM=""

cleanup() {
    [ "$HOST_PID"  -gt 0 ] && kill -KILL "$HOST_PID"  2>/dev/null || true
    [ "$ALICE_PID" -gt 0 ] && kill -KILL "$ALICE_PID" 2>/dev/null || true
    [ "$BOB_PID"   -gt 0 ] && kill -KILL "$BOB_PID"   2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    exec 4>&- 2>/dev/null || true
    exec 6>&- 2>/dev/null || true
    [ -n "$P2P_SHM" ] && rm -f "/dev/shm/${P2P_SHM#/}" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ────────────────────────────────────────────────────────────────
mkfifo "$TEST_DIR/host_in.fifo"
"$MYCHAT" -H -m p2p -n host \
    < "$TEST_DIR/host_in.fifo" > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!
exec 3>"$TEST_DIR/host_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Host SHM:" \
    || fail_harness "Host did not become ready"

P2P_SHM="$(grep "Host SHM:" "$TEST_DIR/host.log" \
    | tail -1 | sed 's/.*Host SHM: //')"
[ -n "$P2P_SHM" ] || fail_harness "Could not extract P2P SHM name"

# ── Start alice (first client) ────────────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 4>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/alice.log" "Successfully joined" \
    || fail "Alice did not join within timeout"

# ── Start bob (second client) — must block while alice holds the slot ──────────
# FD 6 stays open throughout so bob does not get EOF on stdin prematurely.
mkfifo "$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n bob \
    < "$TEST_DIR/bob_in.fifo" > /dev/null 2> "$TEST_DIR/bob.log" &
BOB_PID=$!
exec 6>"$TEST_DIR/bob_in.fifo"

sleep 0.5
if grep -q "Successfully joined" "$TEST_DIR/bob.log" 2>/dev/null; then
    fail "Bob joined while alice was still connected (exclusivity violated)"
fi

# ── Disconnect alice → bob should now be able to join ─────────────────────────
exec 4>&-   # close alice's stdin → EOF → LEAVE → alice exits, freeing the slot
wait_for_line "$TEST_DIR/host.log" "Left:" 30 \
    || true  # best-effort; alice may exit without a clean LEAVE log line

wait_for_line "$TEST_DIR/bob.log" "Successfully joined" 80 \
    || fail "Bob did not join after alice disconnected"

# ── Teardown ──────────────────────────────────────────────────────────────────
exec 6>&-
kill -KILL "$BOB_PID"   2>/dev/null || true
BOB_PID=0
kill -KILL "$HOST_PID"  2>/dev/null || true
HOST_PID=0
kill -KILL "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

echo "PASS"
