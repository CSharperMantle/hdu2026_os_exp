# shellcheck shell=bash

pass() {
	exit 0
}

fail() {
	echo "FAIL: $*" >&2
	exit 1
}

bust() {
	echo "HARNESS BUSTED: $*" >&2
	exit 2
}

step() {
	echo "  >> $*" >&2
}

# Poll $1 every 0.1s until $2 appears, for a maximum of $3 times.
wait_for_line() {
	local file="$1" pattern="$2" max_iters="${3:-30}" i=0
	while [ $i -lt "$max_iters" ]; do
		grep -q "$pattern" "$file" 2>/dev/null && return 0
		sleep 0.1
		i=$((i + 1))
	done
	return 1
}

# Poll $1 until (grep -c $2) >= $3.
wait_for_count() {
	local file="$1" pattern="$2" expected="$3" max_iters="${4:-20}" i=0 count=0
	while [ $i -lt "$max_iters" ]; do
		count=$(grep -c "$pattern" "$file" 2>/dev/null) || count=0
		[ "$count" -ge "$expected" ] && return 0
		sleep 0.1
		i=$((i + 1))
	done
	return 1
}
