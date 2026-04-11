#!/bin/bash
# P2P bidirectional message delivery through SHM half-duplex channel.

set -eo pipefail

source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
	kill -KILL "$HOST_PID" "$BOB_PID" 2>/dev/null || true
	exec 3>&- 4>&- 2>/dev/null || true
	rm -f "/dev/shm/$P2P_SHM" 2>/dev/null || true
	rm -f "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" 2>/dev/null || true
	rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -H -m p2p -n alice <"$TEST_DIR/alice_in.fifo" >/dev/null 2>"$TEST_DIR/alice.log" &
HOST_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/alice.log" "Host SHM:" || bust "Host did not become ready"
P2P_SHM="$(grep "Host SHM:" "$TEST_DIR/alice.log" | sed 's/.*Host SHM: //')"

mkfifo "$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$P2P_SHM" -m p2p -n bob <"$TEST_DIR/bob_in.fifo" >/dev/null 2>"$TEST_DIR/bob.log" &
BOB_PID=$!
exec 4>"$TEST_DIR/bob_in.fifo"

wait_for_line "$TEST_DIR/bob.log" "Successfully joined" || fail "Bob did not join"
wait_for_line "$TEST_DIR/alice.log" "Joined:" || fail "Alice did not see bob's join"

echo "host_to_client" >&3
wait_for_line "$TEST_DIR/bob.log" "host_to_client" || fail "Bob did not receive alice's message"

echo "client_to_host" >&4
wait_for_line "$TEST_DIR/alice.log" "client_to_host" || fail "Alice did not receive bob's message"

exec 4>&-
wait_for_line "$TEST_DIR/alice.log" "Left:" 30 || true
exec 3>&-
kill -INT "$HOST_PID" "$BOB_PID" 2>/dev/null || true
wait "$HOST_PID" 2>/dev/null || true
pass
