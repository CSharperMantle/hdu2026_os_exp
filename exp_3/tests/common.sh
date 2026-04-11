#!/bin/bash
# common.sh - Shared helpers for mychat test scripts.
# Source with: source "$(dirname "$0")/common.sh"
# Do not execute directly.

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

fail_harness() {
    echo "HARNESS ERROR: $*" >&2
    exit 2
}

# ---------------------------------------------------------------------------
# Polling helpers
# ---------------------------------------------------------------------------

# wait_for_line FILE PATTERN [MAX_ITERS=50]
# Poll FILE every 0.1 s until PATTERN appears (grep -q).
# Returns 0 on success, 1 on timeout (~5 s by default).
wait_for_line() {
    local file="$1"
    local pattern="$2"
    local max_iters="${3:-50}"
    local i=0
    while [ "$i" -lt "$max_iters" ]; do
        if grep -q "$pattern" "$file" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}

# wait_for_count FILE PATTERN EXPECTED [MAX_ITERS=100]
# Poll FILE every 0.1 s until (grep -c PATTERN) >= EXPECTED.
# Returns 0 on success, 1 on timeout (~10 s by default).
wait_for_count() {
    local file="$1"
    local pattern="$2"
    local expected="$3"
    local max_iters="${4:-100}"
    local i=0
    local count
    while [ "$i" -lt "$max_iters" ]; do
        count=$(grep -c "$pattern" "$file" 2>/dev/null) || count=0
        if [ "$count" -ge "$expected" ]; then
            return 0
        fi
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}

# strip_ansi FILE
# Remove ANSI SGR escape sequences from FILE in-place.
strip_ansi() {
    local file="$1"
    local tmp
    tmp="$(mktemp)"
    sed 's/\x1b\[[0-9;]*m//g' "$file" > "$tmp" && mv "$tmp" "$file"
}
