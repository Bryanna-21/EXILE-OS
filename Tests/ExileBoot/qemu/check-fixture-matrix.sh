#!/usr/bin/env bash
# Runs every scenario in EXPECTED.txt through the host verifier and compares the outcome.
# This exercises the fixtures with the HOST tool. It is NOT a boot test: it does not run the EFI prekernel.
#
# Usage: check-fixture-matrix.sh /path/to/ExileBootManifest <fixture-dir>
set -u
TOOL="${1:?usage: $0 /path/to/ExileBootManifest <fixture-dir>}"
DIR="${2:?usage: $0 /path/to/ExileBootManifest <fixture-dir>}"
pass=0; fail=0
while read -r scenario manifest kernel anchor expected; do
    case "$scenario" in ''|\#*) continue ;; esac
    out="$("$TOOL" verify "$DIR/$kernel" "$DIR/$manifest" "$DIR/$anchor" 2>&1)"; rc=$?
    case "$expected" in
        Ok) want_rc=0; want_text="OK:" ;;
        BadSignature) want_rc=2; want_text="boot manifest signature is invalid" ;;
        DigestMismatch) want_rc=2; want_text="kernel image digest does not match the boot manifest" ;;
        UnknownKey) want_rc=2; want_text="boot manifest is signed by an unknown key" ;;
        refused-before-verification) want_rc=1; want_text="all zero" ;;
        *) echo "FAIL $scenario: unknown expectation $expected"; fail=$((fail+1)); continue ;;
    esac
    if [ "$rc" -eq "$want_rc" ] && printf '%s' "$out" | grep -qi -- "$want_text"; then
        echo "PASS $scenario: expected $expected, got exit $rc ($(printf '%s' "$out" | head -1))"; pass=$((pass+1))
    else
        echo "FAIL $scenario: expected $expected (exit $want_rc, text '$want_text'), got exit $rc: $out"; fail=$((fail+1))
    fi
done <"$DIR/EXPECTED.txt"
echo "fixture matrix: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
