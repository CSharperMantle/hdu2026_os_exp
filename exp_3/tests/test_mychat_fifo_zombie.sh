#!/bin/bash
# Host detects and reaps a SIGKILL'd client via FIFO probe.
set -euo pipefail
source "$(dirname "$0")/common.sh"
MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
    kill "$HOST_PID" "$ZOMBIE_PID" "$ALICE_PID" 2>/dev/null || true
    exec 3>&- 4>&- 2>/dev/null || true
    rm -f "$HOST_FIFO" 2>/dev/null || true
    rm -f "$TEST_DIR/zombie_in.fifo" "$TEST_DIR/alice_in.fifo" 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m fifo > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!
HOST_FIFO="$(grep "Control FIFO:" "$TEST_DIR/host.log" | sed 's/.*Control FIFO: //')"

mkfifo "$TEST_DIR/zombie_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n zombie < "$TEST_DIR/zombie_in.fifo" > /dev/null 2> "$TEST_DIR/zombie.log" &
ZOMBIE_PID=$!; exec 3>"$TEST_DIR/zombie_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" || fail "Zombie did not join"

kill -KILL "$ZOMBIE_PID" 2>/dev/null || true
ZOMBIE_PID=0
exec 3>&-

wait_for_line "$TEST_DIR/host.log" "Reaped zombie" 80 || fail "Host did not reap zombie"

mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_FIFO" -m fifo -n alice < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!; exec 4>"$TEST_DIR/alice_in.fifo"
wait_for_line "$TEST_DIR/host.log" "Joined:" || fail "Host rejected new client after zombie reap"

exec 4>&-
wait $ALICE_PID 2>/dev/null || true
kill -INT $HOST_PID
wait $HOST_PID 2>/dev/null || true
pass
