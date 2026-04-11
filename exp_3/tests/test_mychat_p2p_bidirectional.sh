#!/bin/bash
# test_mychat_p2p_bidirectional.sh
# Tests: P2P bidirectional message delivery through the SHM half-duplex channel.
# Alice (host) sends to Bob (client) and vice versa; verifies both receive.
#
# NOTE: p2pRecv calls csem.wait with @panic on error, so SIGINT cannot be used
# for clean teardown. The host is force-killed and the SHM is removed manually.
# Usage: test_mychat_p2p_bidirectional.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

TEST_DIR="$(mktemp -d)"
HOST_PID=0
BOB_PID=0
P2P_SHM=""

cleanup() {
    [ "$HOST_PID" -gt 0 ] && kill -KILL "$HOST_PID" 2>/dev/null || true
    [ "$BOB_PID"  -gt 0 ] && kill -KILL "$BOB_PID"  2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    exec 4>&- 2>/dev/null || true
    [ -n "$P2P_SHM" ] && rm -f "/dev/shm/${P2P_SHM#/}" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host (alice) ────────────────────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -H -m p2p -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
HOST_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/alice.log" "Host SHM:" \
    || fail_harness "Host did not become ready"

P2P_SHM="$(grep "Host SHM:" "$TEST_DIR/alice.log" \
    | tail -1 | sed 's/.*Host SHM: //')"
[ -n "$P2P_SHM" ] || fail_harness "Could not extract P2P SHM name"

# ── Start client (bob) ────────────────────────────────────────────────────────
mkfifo "$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n bob \
    < "$TEST_DIR/bob_in.fifo" > /dev/null 2> "$TEST_DIR/bob.log" &
BOB_PID=$!
exec 4>"$TEST_DIR/bob_in.fifo"

wait_for_line "$TEST_DIR/bob.log" "Successfully joined" \
    || fail "Bob did not join within timeout"

# Also wait for alice to process bob's JOIN frame
wait_for_line "$TEST_DIR/alice.log" "Joined:" \
    || fail "Alice did not see bob's join"

# ── Alice → Bob ───────────────────────────────────────────────────────────────
echo "host_to_client" >&3
wait_for_line "$TEST_DIR/bob.log" "host_to_client" 50 \
    || fail "Bob did not receive alice's message"

# ── Bob → Alice ───────────────────────────────────────────────────────────────
echo "client_to_host" >&4
wait_for_line "$TEST_DIR/alice.log" "client_to_host" 50 \
    || fail "Alice did not receive bob's message"

# ── Teardown: close bob stdin first, then alice ───────────────────────────────
exec 4>&-
wait_for_line "$TEST_DIR/alice.log" "Left:" 30 \
    || true   # best-effort; LEAVE may not arrive if semaphore is timing out

# Close alice stdin; alice may be stuck in recvLoop — force-kill handles it.
exec 3>&-
sleep 0.2
kill -KILL "$HOST_PID" 2>/dev/null || true
HOST_PID=0
kill -KILL "$BOB_PID"  2>/dev/null || true
BOB_PID=0

echo "PASS"
