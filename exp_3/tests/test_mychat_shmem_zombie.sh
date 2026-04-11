#!/bin/bash
# Host stays alive and alice communicates after zombie client's shm is leaked.

set -eo pipefail

source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
TEST_DIR="$(mktemp -d)"

cleanup() {
	kill -INT "$HOST_PID" "$ZOMBIE_PID" "$ALICE_PID" 2>/dev/null || true
	exec 3>&- 4>&- 2>/dev/null || true
	rm -f "/dev/shm/$HOST_SHM" 2>/dev/null || true
	rm -f "$TEST_DIR/zombie_in.fifo" "$TEST_DIR/alice_in.fifo" 2>/dev/null || true
	rm -rf "$TEST_DIR"
}
trap cleanup EXIT

"$MYCHAT" -H -m shmem >/dev/null 2>"$TEST_DIR/host.log" &
HOST_PID=$!
wait_for_line "$TEST_DIR/host.log" "Host SHM:" || bust "Host did not become ready"
HOST_SHM="$(grep "Host SHM:" "$TEST_DIR/host.log" | sed 's/.*Host SHM: //')"

mkfifo "$TEST_DIR/zombie_in.fifo"
"$MYCHAT" -C "$HOST_SHM" -m shmem -n zombie <"$TEST_DIR/zombie_in.fifo" >/dev/null 2>"$TEST_DIR/zombie.log" &
ZOMBIE_PID=$!
disown "$ZOMBIE_PID"
exec 3>"$TEST_DIR/zombie_in.fifo"
wait_for_count "$TEST_DIR/host.log" "Joined:" 1 20 || fail "Zombie did not join"

kill -KILL "$ZOMBIE_PID" 2>/dev/null || true
exec 3>&-

mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C "$HOST_SHM" -m shmem -n alice <"$TEST_DIR/alice_in.fifo" >/dev/null 2>"$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 4>"$TEST_DIR/alice_in.fifo"
wait_for_count "$TEST_DIR/host.log" "Joined:" 2 20 || fail "Alice did not join"

for i in {0..5}; do
	echo "alive_check_$i" >&4
	wait_for_line "$TEST_DIR/alice.log" "alive_check_$i" || fail "Alice message $i failed"
done

wait_for_line "$TEST_DIR/host.log" "Reaped zombie client: zombie" || fail "Host did not reap zombie"

exec 4>&-
wait $ALICE_PID 2>/dev/null || true
kill -INT "$HOST_PID"
wait $HOST_PID 2>/dev/null || true
pass
