#!/bin/bash
# Basic join/message/leave cycle for MQ.
set -euo pipefail
source "$(dirname "$0")/common.sh"
MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
    kill "$HOST_PID" "$ALICE_PID" 2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    rm -f "/dev/mqueue/$HOST_MQ" 2>/dev/null || true
    rm -f "$TEST_DIR/alice_in.fifo" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m mq > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!
wait_for_line "$TEST_DIR/host.log" "Host MQ:" || bust "Host did not become ready"
HOST_MQ="$(grep "Host MQ:" "$TEST_DIR/host.log" | sed 's/.*Host MQ: //')"
[ -e "/dev/mqueue/$HOST_MQ" ] || bust "Host MQ not created"

mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_MQ" -m mq -n alice < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!; exec 3>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" || fail "Alice did not join"
echo "ping_mq" >&3
wait_for_line "$TEST_DIR/host.log" "ping_mq" || fail "Host did not receive ping"

exec 3>&-
wait_for_line "$TEST_DIR/host.log" "Left:" || fail "Host did not log alice's departure"
wait $ALICE_PID 2>/dev/null || true

kill -INT $HOST_PID
wait $HOST_PID 2>/dev/null || true
[ -e "/dev/mqueue/$HOST_MQ" ] && fail "Host MQ not removed after host exit"
pass
