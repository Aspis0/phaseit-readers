#!/usr/bin/env bash
# Fails when a tracked binary file could be rewritten by git's end-of-line filter.
#
# Git converts line endings only in files it believes are text, and a NUL byte
# in the first 8000 bytes is what makes git call a file binary. That rule
# misses the fixture that made .gitattributes necessary in the first place - a
# constant u16 plane whose every second byte is 0x0A - so every file under
# tests/fixtures/ counts as binary here as well.
#
# A binary file must carry `text: unset`: that is what stops git from
# rewriting it. `text: set` and `text: unspecified` are both failures.

set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1

# True when the file holds a NUL byte, which is how git decides that a file is
# binary: the count of NUL bytes in the file is not zero.
has_nul() {
    local file=$1 nul_bytes
    nul_bytes=$(tr -dc '\000' < "$file" | wc -c)
    (( nul_bytes > 0 ))
}

checked=0
failed=0

while IFS= read -r -d '' file; do
    if [[ $file != tests/fixtures/* ]] && ! has_nul "$file"; then
        continue
    fi
    checked=$((checked + 1))
    attribute=$(git check-attr text -- "$file")
    case $attribute in
        *": text: unset") ;;
        *)
            echo "FAIL $file: $attribute"
            failed=$((failed + 1))
            ;;
    esac
done < <(git ls-files -z)

if [ "$failed" -ne 0 ]; then
    echo "$failed of $checked binary file(s) are not marked -text in .gitattributes"
    exit 1
fi
echo "OK: all $checked binary file(s) are marked -text"
