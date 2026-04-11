#!/bin/bash
# Host broadcasts messages to all clients.
set -euo pipefail
source "$(dirname "$0")/common.sh"
MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
    kill "$HOST_PID" "$ALICE_PID" "$BOB_PID" "$CHARLIE_PID" 2>/dev/null || true
    exec 3>&- 4>&- 5>&- 2>/dev/null || true
    rm -f "/dev/mqueue/$HOST_MQ" /dev/mqueue/mychat-client-* 2>/dev/null || true
    rm -f "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m mq > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!
wait_for_line "$TEST_DIR/host.log" "Host MQ:" || bust "Host did not become ready"
HOST_MQ="$(grep "Host MQ:" "$TEST_DIR/host.log" | sed 's/.*Host MQ: //')"

mkfifo "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo"
"$MYCHAT" -C "$HOST_MQ" -m mq -n alice < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!; exec 3>"$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_MQ" -m mq -n bob   < "$TEST_DIR/bob_in.fifo"   > /dev/null 2> "$TEST_DIR/bob.log"   &
BOB_PID=$!;   exec 4>"$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$HOST_MQ" -m mq -n charlie < "$TEST_DIR/charlie_in.fifo" > /dev/null 2> "$TEST_DIR/charlie.log" &
CHARLIE_PID=$!; exec 5>"$TEST_DIR/charlie_in.fifo"

wait_for_count "$TEST_DIR/host.log" "Joined:" 3 20 || fail "Not all clients joined"

echo "from_alice" >&3
echo "from_bob"   >&4

wait_for_line "$TEST_DIR/bob.log"     "from_alice" || fail "Bob missing alice's msg"
wait_for_line "$TEST_DIR/charlie.log" "from_alice" || fail "Charlie missing alice's msg"
wait_for_line "$TEST_DIR/alice.log"   "from_bob"   || fail "Alice missing bob's msg"
wait_for_line "$TEST_DIR/charlie.log" "from_bob"   || fail "Charlie missing bob's msg"

exec 3>&- 4>&- 5>&-
wait_for_count "$TEST_DIR/host.log" "Left:" 3 20 || fail "Not all clients left"
kill -INT $HOST_PID
wait $HOST_PID 2>/dev/null || true
pass
