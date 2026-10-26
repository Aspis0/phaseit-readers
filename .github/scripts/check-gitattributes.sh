#!/usr/bin/env bash
# Checks that the end-of-line policy in .gitattributes matches the tracked files.
#
# Git rewrites line endings only in files it believes are text, and it believes
# that when the file holds no NUL byte. That test misses the fixture that made
# .gitattributes necessary in the first place - a constant u16 plane whose
# bytes are 0xBC 0x0A, so every second byte is a line ending - and it misses
# every file whose bytes are not UTF-8. So the script decides for itself what
# is binary:
#
#   - the file holds a NUL byte;
#   - the file holds a control byte that is neither tab, newline nor carriage
#     return;
#   - the file is not valid UTF-8.
#
# Two rules follow from that:
#
#   - a binary file must carry `text: unset` (-text): git then leaves its
#     bytes alone. `text: set` and `text: unspecified` are both failures.
#   - a text file under tests/fixtures/ must carry `text: set` and `eol: lf`:
#     the fixture generators write the manifests in text mode, which is CRLF
#     on Windows, so a regenerated manifest would otherwise differ from the
#     committed one on every platform.
#
# The check is not vacuous: it fails when it finds no binary file at all, so a
# tree without fixtures cannot pass with the rule untested.

set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1

if ! command -v iconv > /dev/null 2>&1; then
    echo "FAIL: iconv is not available, so the binary check cannot run" >&2
    exit 1
fi

# The bytes an end-of-line filter must not touch: the C0 controls other than
# tab, newline and carriage return, and DEL. Printable ASCII and the high
# bytes of a UTF-8 sequence are text.
control_bytes() {
    LC_ALL=C tr -d '\11\12\15\40-\176\200-\377' < "$1" | wc -c
}

# True when the file is binary: it holds a byte an end-of-line filter could
# corrupt, or its bytes are not UTF-8.
is_binary() {
    local file=$1
    if [ "$(control_bytes "$file")" -gt 0 ]; then
        return 0
    fi
    iconv -f UTF-8 -t UTF-8 < "$file" > /dev/null 2>&1 && return 1
    return 0
}

# The value git gives one attribute of one file.
attr_value() {
    local line
    line=$(git check-attr "$2" -- "$1")
    printf '%s' "${line##*: }"
}

binary_count=0
fixture_text_count=0
failed=0

while IFS= read -r -d '' file; do
    if [ ! -f "$file" ]; then
        continue
    fi
    if is_binary "$file"; then
        binary_count=$((binary_count + 1))
        text=$(attr_value "$file" text)
        if [ "$text" != "unset" ]; then
            echo "FAIL $file: text: $text, want unset (-text)"
            failed=$((failed + 1))
        fi
    elif [ "${file#tests/fixtures/}" != "$file" ]; then
        fixture_text_count=$((fixture_text_count + 1))
        text=$(attr_value "$file" text)
        eol=$(attr_value "$file" eol)
        if [ "$text" != "set" ] || [ "$eol" != "lf" ]; then
            echo "FAIL $file: text: $text, eol: $eol, want text: set and eol: lf"
            failed=$((failed + 1))
        fi
    fi
done < <(git ls-files -z)

if [ "$binary_count" -eq 0 ]; then
    echo "FAIL: no binary file was found, so the -text rule was not checked at all"
    exit 1
fi

if [ "$failed" -ne 0 ]; then
    echo "$failed of $((binary_count + fixture_text_count)) checked file(s) do not match .gitattributes"
    exit 1
fi

echo "OK: $binary_count binary file(s) are marked -text, $fixture_text_count text fixture(s) are marked text eol=lf"
