#!/bin/bash
# Three clients send one message each simultaneously; verify all arrive.
set -euo pipefail
source "$(dirname "$0")/common.sh"
MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
    kill "$HOST_PID" "$ALICE_PID" "$BOB_PID" "$CHARLIE_PID" 2>/dev/null || true
    exec 3>&- 4>&- 5>&- 2>/dev/null || true
    rm -f "$HOST_FIFO" 2>/dev/null || true
    rm -f "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m fifo > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!
HOST_FIFO="$(grep "Control FIFO:" "$TEST_DIR/host.log" | sed 's/.*Control FIFO: //')"

mkfifo "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n alice < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!; exec 3>"$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n bob   < "$TEST_DIR/bob_in.fifo"   > /dev/null 2> "$TEST_DIR/bob.log"   &
BOB_PID=$!;   exec 4>"$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n charlie < "$TEST_DIR/charlie_in.fifo" > /dev/null 2> "$TEST_DIR/charlie.log" &
CHARLIE_PID=$!; exec 5>"$TEST_DIR/charlie_in.fifo"

wait_for_count "$TEST_DIR/host.log" "Joined:" 3 20 || fail "Not all clients joined"

{ echo "STRESS_alice_0"; } >&3 &
FA=$!
{ echo "STRESS_bob_0"; } >&4 &
FB=$!
{ echo "STRESS_charlie_0"; } >&5 &
FC=$!
wait $FA $FB $FC

wait_for_count "$TEST_DIR/host.log" "STRESS_" 3 100 || {
    actual=$(grep -c "STRESS_" "$TEST_DIR/host.log" 2>/dev/null) || actual=0
    fail "Expected 3 STRESS messages, got $actual"
}

grep -q "Cannot handle frame" "$TEST_DIR/host.log" && fail "Frame parsing errors"

exec 3>&- 4>&- 5>&-
kill -INT $HOST_PID
wait $HOST_PID 2>/dev/null || true
pass
