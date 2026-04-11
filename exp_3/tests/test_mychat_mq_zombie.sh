#!/bin/bash
# test_mychat_mq_zombie.sh
# Tests: MQ zombie detection via testMqExists.
# The MQ zombie probe fires when the client's MQ is gone (mq_open fails).
# Trigger: SIGKILL the client (defers don't run), then manually remove the MQ
# from /dev/mqueue/ to simulate the missing mq_unlink.
# Usage: test_mychat_mq_zombie.sh path/to/mychat
# Exit 0: pass | 1: assertion failed | 2: harness error
set -euo pipefail

# shellcheck source=common.sh
source "$(dirname "$0")/common.sh"

MYCHAT="${1:?Usage: $0 path/to/mychat}"
[ -x "$MYCHAT" ] || fail_harness "Not executable: $MYCHAT"

TEST_DIR="$(mktemp -d)"
HOST_PID=0
ZOMBIE_PID=0
ALICE_PID=0

cleanup() {
    [ "$HOST_PID"   -gt 0 ] && kill "$HOST_PID"   2>/dev/null || true
    [ "$ZOMBIE_PID" -gt 0 ] && kill "$ZOMBIE_PID" 2>/dev/null || true
    [ "$ALICE_PID"  -gt 0 ] && kill "$ALICE_PID"  2>/dev/null || true
    exec 3>&- 2>/dev/null || true
    exec 4>&- 2>/dev/null || true
    rm -f /dev/mqueue/mychat-host 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# ── Start host ──────────────────────────────────────────────────────────────
"$MYCHAT" -H -m mq > /dev/null 2> "$TEST_DIR/host.log" &
HOST_PID=$!

wait_for_line "$TEST_DIR/host.log" "Host MQ:" \
    || fail_harness "Host did not become ready"

# ── Start zombie client ──────────────────────────────────────────────────────
mkfifo "$TEST_DIR/zombie_in.fifo"
"$MYCHAT" -C /mychat-host -m mq -n zombie \
    < "$TEST_DIR/zombie_in.fifo" > /dev/null 2> "$TEST_DIR/zombie.log" &
ZOMBIE_PID=$!
exec 3>"$TEST_DIR/zombie_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" \
    || fail_harness "Zombie client did not join"

# ── Kill zombie; manually remove its MQ (simulates missing mq_unlink) ────────
# SIGKILL prevents defers from running → MQ not unlinked automatically.
kill -KILL "$ZOMBIE_PID" 2>/dev/null || true
ZOMBIE_PID=0
exec 3>&-
# Remove the zombie's client MQ from /dev/mqueue to trigger probe detection.
rm -f "/dev/mqueue/mychat-client-$ZOMBIE_PID" 2>/dev/null || true
# Glob-remove any leftover client MQs (ZOMBIE_PID is 0 after reset above;
# use a pattern that matches all client MQs except the host).
for mq_file in /dev/mqueue/mychat-client-*; do
    rm -f "$mq_file" 2>/dev/null || true
done

# ── Wait for zombie reap (probe interval 500 ms; give 8 s) ───────────────────
wait_for_line "$TEST_DIR/host.log" "Reaped zombie" 80 \
    || fail "Host did not reap zombie client within timeout"

# ── Verify host is still operational ─────────────────────────────────────────
mkfifo "$TEST_DIR/alice_in.fifo"
"$MYCHAT" -C /mychat-host -m mq -n alice \
    < "$TEST_DIR/alice_in.fifo" > /dev/null 2> "$TEST_DIR/alice.log" &
ALICE_PID=$!
exec 4>"$TEST_DIR/alice_in.fifo"

wait_for_line "$TEST_DIR/host.log" "Joined:" 50 \
    || fail "Host rejected new client after zombie reap"

exec 4>&-
wait "$ALICE_PID" 2>/dev/null || true
ALICE_PID=0

kill -INT "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=0

echo "PASS"
