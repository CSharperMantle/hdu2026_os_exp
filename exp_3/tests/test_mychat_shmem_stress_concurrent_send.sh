#!/bin/bash
# Three clients send one message each simultaneously; verify all arrive.

set -eo pipefail

source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

N_MSG_EACH=10

cleanup() {
	kill -INT "$HOST_PID" "$ALICE_PID" "$BOB_PID" "$CHARLIE_PID" 2>/dev/null || true
	exec 3>&- 4>&- 5>&- 2>/dev/null || true
	rm -f "/dev/shm/$HOST_SHM" 2>/dev/null || true
	rm -f "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo" 2>/dev/null || true
	rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m shmem >/dev/null 2>"$TEST_DIR/host.log" &
HOST_PID=$!
wait_for_line "$TEST_DIR/host.log" "Host SHM:" || bust "Host did not become ready"
HOST_SHM="$(grep "Host SHM:" "$TEST_DIR/host.log" | sed 's/.*Host SHM: //')"

mkfifo "$TEST_DIR/alice_in.fifo" "$TEST_DIR/bob_in.fifo" "$TEST_DIR/charlie_in.fifo"
"$MYCHAT" -C "$HOST_SHM" -m shmem -n alice <"$TEST_DIR/alice_in.fifo" >/dev/null 2>"$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 3>"$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_SHM" -m shmem -n bob <"$TEST_DIR/bob_in.fifo" >/dev/null 2>"$TEST_DIR/bob.log" &
BOB_PID=$!
exec 4>"$TEST_DIR/bob_in.fifo"
"$MYCHAT" -C "$HOST_SHM" -m shmem -n charlie <"$TEST_DIR/charlie_in.fifo" >/dev/null 2>"$TEST_DIR/charlie.log" &
CHARLIE_PID=$!
exec 5>"$TEST_DIR/charlie_in.fifo"

wait_for_count "$TEST_DIR/host.log" "Joined:" 3 20 || fail "Not all clients joined"

{
	for ((i = 1; i <= N_MSG_EACH; i++)); do
		echo "STRESS_alice_$i"
		sleep 0
	done
} >&3 &
FA=$!
{
	for ((i = 1; i <= N_MSG_EACH; i++)); do
		echo "STRESS_bob_$i"
		sleep 0
	done
} >&4 &
FB=$!
{
	for ((i = 1; i <= N_MSG_EACH; i++)); do
		echo "STRESS_charlie_$i"
		sleep 0
	done
} >&5 &
FC=$!
wait "$FA" "$FB" "$FC"

exec 3>&- 4>&- 5>&-
wait_for_count "$TEST_DIR/host.log" "STRESS_" "$((N_MSG_EACH * 3))" 100 || {
	ACTUAL=$(grep -c "STRESS_" "$TEST_DIR/host.log" 2>/dev/null) || ACTUAL=0
	MSG="$(printf "%s\n  -- host.log --\n%s\n  --------------\n" "Expected $((N_MSG_EACH * 3)) STRESS messages, got $ACTUAL." "$(cat "$TEST_DIR/host.log")")"
	fail "$MSG"
}

grep -q "Cannot handle frame" "$TEST_DIR/host.log" && fail "Frame parsing errors"

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
pass
