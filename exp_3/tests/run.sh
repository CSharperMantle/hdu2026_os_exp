#!/bin/sh

MYCHAT="${1:?usage: $0 path/to/mychat}"

BASEDIR="$(dirname "$0")"

total=0
n_passed=0
failed_cmds=""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
RESET_COLOR='\033[0m'

print_case_output() {
	echo "  --- Output ---"
	echo "$1"
}

for f in "$BASEDIR"/test_*.sh; do
	total=$((total + 1))

	cmd_lit="$f $MYCHAT"
	output=$("$f" "$MYCHAT" 2>&1 </dev/null)
	exit_code=$?

	passed=0
	reason=''
	if [ "$exit_code" -eq 0 ]; then
		passed=1
	elif [ "$exit_code" -eq 1 ]; then
		reason="Test failed; got exit code $exit_code"
	else
		reason="Busted test harness; got exit code $exit_code"
	fi

	if [ $passed = 1 ]; then
		n_passed=$((n_passed + 1))
		printf "${GREEN}[PASS]${RESET_COLOR} %s\n" "$f"
	else
		printf "${RED}[FAIL]${RESET_COLOR} %s\n" "$f"
		echo "  Reason: $reason"
		print_case_output "$output"
		failed_cmds=$(printf '%s\t%s\nx' "${failed_cmds}" "${cmd_lit}")
		failed_cmds=${failed_cmds%x}
	fi
done

echo ''
echo "Summary: $n_passed / $total tests passed"

if [ "$n_passed" -eq "$total" ]; then
	exit 0
else
	if [ -n "$failed_cmds" ]; then
		echo ''
		echo "FAILURES:"
		echo "$failed_cmds"
	fi
	exit 1
fi
