#!/bin/bash
# P2P enforces single-client exclusivity; second client blocks until first disconnects.
set -euo pipefail
source "$(dirname "$0")/common.sh"
MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"
P2P_SHM=""

cleanup() {
    kill -KILL "$HOST_PID" "$ALICE_PID" "$BOB_PID" 2>/dev/null || true
    exec 3>&- 4>&- 6>&- 2>/dev/null || true
    rm -f "/dev/shm/$P2P_SHM" 2>/dev/null || true
    rm -f "$TEST_DIR/host_in.fifo" "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkfifo "$TEST_DIR/host_in.fifo"
"$MYCHAT" -H -m p2p -n host < "$TEST_DIR/host_in.fifo" > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!; exec 3>"$TEST_DIR/host_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Host SHM:" || bust "Host did not become ready"
P2P_SHM="$(grep "Host SHM:" "$TEST_DIR/host.log" | sed 's/.*Host SHM: //')"

mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n alice < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!; exec 4>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/alice.log" "Successfully joined" || fail "Alice did not join"

mkfifo "$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n bob < "$TEST_DIR/bob_in.fifo" > /dev/null 2> "$TEST_DIR/bob.log" &
BOB_PID=$!; exec 6>"$TEST_DIR/bob_in.fifo"

sleep 0.5
grep -q "Successfully joined" "$TEST_DIR/bob.log" && fail "Bob joined while alice was connected (exclusivity violated)"

exec 4>&-
wait_for_line "$TEST_DIR/host.log" "Left:" 30 || true

kill -KILL $BOB_PID $HOST_PID $ALICE_PID 2>/dev/null || true
pass
