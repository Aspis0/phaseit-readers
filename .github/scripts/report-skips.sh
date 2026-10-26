#!/usr/bin/env bash
# Lists the tests that skipped, in the job summary.
#
# Usage: report-skips.sh <test-log>
#
# A skipped test still counts as passed in the cargo summary, so the only way
# to see what did not run is to read the "skipping:" messages the tests print
# and repeat them. The summary carries the number of "skipping:" messages and
# the distinct reasons behind them, each with the number of tests that skipped
# for it. The script reports and nothing else: the caller keeps cargo's exit
# code.
#
# The script also checks its own arithmetic: the number of messages and the
# number of rows in the table come from two different tools, so a disagreement
# between them means the report cannot be trusted. The script says so and
# exits non-zero. The caller ignores that status - cargo's exit code is what
# decides the run - but a report that does not add up must not pass silently.

set -uo pipefail

log=${1:-}
summary=${GITHUB_STEP_SUMMARY:-}

if [[ -z $log || ! -f $log ]]; then
    echo "report-skips: no test log at '${log}', nothing to report" >&2
    exit 0
fi

# Every "skipping:" message in the log, counted as messages and not as lines:
# a line that carries two of them is two skips.
total=$(grep -o 'skipping:' "$log" | wc -l || true)

# One reason per message. The test harness writes its own progress without a
# newline, so a reason can be cut off its "skipping:" prefix and land on the
# next line; the two are joined back here. A pipe in a reason is escaped so
# that it cannot break the table.
reasons=$(
    awk '
        {
            s = $0
            while (match(s, /skipping:/)) {
                rest = substr(s, RSTART + 9)
                s = substr(s, RSTART + 9)
                sub(/^[[:space:]]+/, "", rest)
                if (rest != "") {
                    print rest
                    continue
                }
                # The reason was cut off at the end of the line: it lands on
                # the next one, which may itself start another message.
                if ((getline nl) <= 0) {
                    print ""
                    break
                }
                cut = index(nl, "skipping:")
                if (cut > 0) {
                    head = substr(nl, 1, cut - 1)
                    sub(/^[[:space:]]+/, "", head)
                    print head
                    s = substr(nl, cut)
                    continue
                }
                sub(/^[[:space:]]+/, "", nl)
                print nl
                break
            }
        }
    ' "$log" | sed 's/|/\\|/g' | sort | uniq -c | sort -rn
)

# The rows must account for every message.
rows=$(printf '%s\n' "$reasons" | awk 'NF { sum += $1 } END { print sum + 0 }')
if [ "$rows" -ne "$total" ]; then
    echo "report-skips: the table counts $rows skip(s), the log holds $total 'skipping:' message(s)" >&2
    exit 1
fi

report=$(
    echo "### Skipped tests"
    echo
    if [ "$total" -eq 0 ]; then
        echo "No test skipped: every test ran."
    else
        echo "\`cargo test\` printed **$total** \`skipping:\` message(s). A skipped test counts as passed in the cargo summary, so these tests did not run:"
        echo
        echo "| count | reason |"
        echo "| ---: | --- |"
        printf '%s\n' "$reasons" | awk 'NF { count = $1; $1 = ""; sub(/^ +/, ""); printf "| %s | %s |\n", count, $0 }'
    fi
    echo
)

if [ -n "$summary" ]; then
    printf '%s\n' "$report" >> "$summary"
else
    echo "report-skips: GITHUB_STEP_SUMMARY is unset, writing the report to stdout" >&2
    printf '%s\n' "$report"
fi
