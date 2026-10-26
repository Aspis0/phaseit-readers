#!/usr/bin/env bash
# Lists the tests that skipped, in the job summary.
#
# Usage: report-skips.sh <test-log>
#
# A skipped test still counts as passed in the cargo summary, so the only way
# to see what did not run is to read the "skipping:" messages the tests print
# and repeat them. The summary carries the number of "skipping:" lines and the
# distinct reasons behind them, each with the number of tests that skipped for
# it. The script reports and nothing else: the caller keeps cargo's exit code.

set -uo pipefail

log=${1:-}
summary=${GITHUB_STEP_SUMMARY:-}

if [[ -z $log || ! -f $log ]]; then
    echo "report-skips: no test log at '${log}', nothing to report" >&2
    exit 0
fi

if [[ -z $summary ]]; then
    echo "report-skips: GITHUB_STEP_SUMMARY is unset, writing the report to stdout" >&2
    summary=/dev/stdout
fi

total=$(grep -c 'skipping:' "$log" || true)

# One reason per skip. The test harness writes its own progress without a
# newline, so a reason can be cut off its "skipping:" prefix and land on the
# next line; the two are joined back here. A pipe in a reason is escaped so
# that it cannot break the table.
reasons=$(
    awk '
        index($0, "skipping:") {
            if (pending) { print ""; pending = 0 }
            rest = substr($0, index($0, "skipping:") + 9)
            sub(/^[[:space:]]+/, "", rest)
            if (rest == "") { pending = 1 } else { print rest }
            next
        }
        pending { sub(/^[[:space:]]+/, ""); print; pending = 0 }
        END { if (pending) print "" }
    ' "$log" | sed 's/|/\\|/g' | sort | uniq -c | sort -rn
)

{
    echo "### Skipped tests"
    echo
    if [[ $total -eq 0 ]]; then
        echo "No test skipped: every test ran."
    else
        echo "\`cargo test\` printed **$total** \`skipping:\` message(s). A skipped test counts as passed in the cargo summary, so these tests did not run:"
        echo
        echo "| count | reason |"
        echo "| ---: | --- |"
        printf '%s\n' "$reasons" | awk 'NF { count = $1; $1 = ""; sub(/^ +/, ""); printf "| %s | %s |\n", count, $0 }'
    fi
    echo
} >> "$summary"
