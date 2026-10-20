#!/usr/bin/env bash
# End-to-end tamper tests for the Exile boot manifest tool.
#
# Usage: test-boot-integrity.sh /path/to/ExileBootManifest
#
# Signs a stand-in kernel image, then checks that the untouched image verifies and that every kind of
# tampering is rejected with exit status 2. Exit status 0 means every check behaved as required.

set -u

TOOL="${1:-}"
if [ -z "$TOOL" ] || [ ! -x "$TOOL" ]; then
    echo "usage: $0 /path/to/ExileBootManifest" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0

ok() {
    pass=$((pass + 1))
    echo "PASS: $1"
}

bad() {
    fail=$((fail + 1))
    echo "FAIL: $1"
}

# expect_status <expected exit status> <description> <command...>
expect_status() {
    expected="$1"
    description="$2"
    shift 2
    "$@" >"$WORK/out.txt" 2>&1
    actual=$?
    if [ "$actual" -eq "$expected" ]; then
        ok "$description"
    else
        bad "$description (expected exit $expected, got $actual)"
        sed 's/^/    /' "$WORK/out.txt"
    fi
}

# Flips one byte of a file (XOR 0xff) at the given offset, writing the result to a new file.
flip_byte() {
    python3 -I -c '
import sys
data = bytearray(open(sys.argv[1], "rb").read())
data[int(sys.argv[3])] ^= 0xFF
open(sys.argv[2], "wb").write(data)
' "$1" "$2" "$3"
}

head -c 100000 /dev/urandom >"$WORK/kernel.bin"
"$TOOL" keygen "$WORK/seed.key" "$WORK/pub.key" >/dev/null
"$TOOL" keygen "$WORK/other-seed.key" "$WORK/other-pub.key" >/dev/null

expect_status 0 "sign a kernel image" "$TOOL" sign "$WORK/kernel.bin" "$WORK/seed.key" "$WORK/manifest.bin" 7
expect_status 0 "untouched kernel verifies" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest.bin" "$WORK/pub.key"

if [ "$(wc -c <"$WORK/manifest.bin")" -eq 128 ]; then
    ok "manifest is exactly 128 bytes"
else
    bad "manifest is not 128 bytes"
fi

if [ "$(head -c 4 "$WORK/manifest.bin")" = "EXLB" ]; then
    ok "manifest starts with the bytes EXLB"
else
    bad "manifest magic is not EXLB"
fi

# --- Tampering with the kernel image ---
flip_byte "$WORK/kernel.bin" "$WORK/kernel-first.bin" 0
flip_byte "$WORK/kernel.bin" "$WORK/kernel-middle.bin" 50000
flip_byte "$WORK/kernel.bin" "$WORK/kernel-last.bin" 99999
expect_status 2 "kernel with a modified first byte is rejected" "$TOOL" verify "$WORK/kernel-first.bin" "$WORK/manifest.bin" "$WORK/pub.key"
expect_status 2 "kernel with a modified middle byte is rejected" "$TOOL" verify "$WORK/kernel-middle.bin" "$WORK/manifest.bin" "$WORK/pub.key"
expect_status 2 "kernel with a modified last byte is rejected" "$TOOL" verify "$WORK/kernel-last.bin" "$WORK/manifest.bin" "$WORK/pub.key"

head -c 99999 "$WORK/kernel.bin" >"$WORK/kernel-short.bin"
cp "$WORK/kernel.bin" "$WORK/kernel-long.bin"
printf '\0' >>"$WORK/kernel-long.bin"
: >"$WORK/kernel-empty.bin"
expect_status 2 "truncated kernel is rejected" "$TOOL" verify "$WORK/kernel-short.bin" "$WORK/manifest.bin" "$WORK/pub.key"
expect_status 2 "kernel with an appended byte is rejected" "$TOOL" verify "$WORK/kernel-long.bin" "$WORK/manifest.bin" "$WORK/pub.key"
expect_status 2 "empty kernel is rejected" "$TOOL" verify "$WORK/kernel-empty.bin" "$WORK/manifest.bin" "$WORK/pub.key"

# --- Tampering with the manifest: every single byte must matter ---
manifest_failures=0
offset=0
while [ "$offset" -lt 128 ]; do
    flip_byte "$WORK/manifest.bin" "$WORK/manifest-flipped.bin" "$offset"
    if "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-flipped.bin" "$WORK/pub.key" >/dev/null 2>&1; then
        manifest_failures=$((manifest_failures + 1))
        echo "    manifest byte $offset can be changed without detection"
    fi
    offset=$((offset + 1))
done
if [ "$manifest_failures" -eq 0 ]; then
    ok "changing any single byte of the manifest is rejected (128 of 128)"
else
    bad "$manifest_failures manifest bytes can be changed without detection"
fi

# --- Missing, truncated and extended manifests ---
: >"$WORK/manifest-empty.bin"
head -c 127 "$WORK/manifest.bin" >"$WORK/manifest-short.bin"
cp "$WORK/manifest.bin" "$WORK/manifest-long.bin"
printf '\0' >>"$WORK/manifest-long.bin"
expect_status 2 "empty manifest is rejected" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-empty.bin" "$WORK/pub.key"
expect_status 2 "truncated manifest is rejected" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-short.bin" "$WORK/pub.key"
expect_status 2 "manifest with a trailing byte is rejected" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-long.bin" "$WORK/pub.key"
expect_status 1 "missing manifest file is an error" "$TOOL" verify "$WORK/kernel.bin" "$WORK/does-not-exist.bin" "$WORK/pub.key"

# --- Untrusted signers ---
expect_status 2 "manifest checked against a different public key is rejected" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest.bin" "$WORK/other-pub.key"

head -c 100000 /dev/urandom >"$WORK/evil-kernel.bin"
"$TOOL" sign "$WORK/evil-kernel.bin" "$WORK/other-seed.key" "$WORK/evil-manifest.bin" >/dev/null
expect_status 0 "attacker kernel verifies against the attacker's own key (control)" "$TOOL" verify "$WORK/evil-kernel.bin" "$WORK/evil-manifest.bin" "$WORK/other-pub.key"
expect_status 2 "attacker kernel and manifest are rejected by the trusted key" "$TOOL" verify "$WORK/evil-kernel.bin" "$WORK/evil-manifest.bin" "$WORK/pub.key"
expect_status 2 "attacker kernel with the genuine manifest is rejected" "$TOOL" verify "$WORK/evil-kernel.bin" "$WORK/manifest.bin" "$WORK/pub.key"

# --- Key file hygiene ---
head -c 31 "$WORK/pub.key" >"$WORK/pub-short.key"
expect_status 1 "public key file of the wrong length is an error" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest.bin" "$WORK/pub-short.key"
expect_status 1 "signing with a seed of the wrong length is an error" "$TOOL" sign "$WORK/kernel.bin" "$WORK/pub-short.key" "$WORK/never.bin"
if [ ! -e "$WORK/never.bin" ]; then
    ok "no manifest is written when signing fails"
else
    bad "a manifest was written even though signing failed"
fi

# --- Independent cross-check with OpenSSL, and domain separation ---
if command -v openssl >/dev/null 2>&1 && openssl list -public-key-algorithms 2>/dev/null | grep -qi ed25519; then
    # Wrap the raw seed in a PKCS#8 header so OpenSSL can sign with the same key.
    python3 -I -c '
import sys
seed = open(sys.argv[1], "rb").read()
open(sys.argv[2], "wb").write(bytes.fromhex("302e020100300506032b657004220420") + seed)
' "$WORK/seed.key" "$WORK/seed.der"
    openssl pkey -inform DER -in "$WORK/seed.der" -out "$WORK/seed.pem" 2>/dev/null

    # Re-sign the signed region WITHOUT the domain prefix, using OpenSSL, and splice it into the manifest.
    head -c 64 "$WORK/manifest.bin" >"$WORK/region.bin"
    openssl pkeyutl -sign -rawin -inkey "$WORK/seed.pem" -in "$WORK/region.bin" -out "$WORK/nodomain.sig" 2>/dev/null
    cat "$WORK/region.bin" "$WORK/nodomain.sig" >"$WORK/manifest-nodomain.bin"
    expect_status 2 "valid Ed25519 signature without the signing domain is rejected" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-nodomain.bin" "$WORK/pub.key"

    # Re-sign with the domain prefix using OpenSSL: an independent implementation must produce a manifest we accept.
    { printf 'EXILE-BOOT-MANIFEST-V2\0'; cat "$WORK/region.bin"; } >"$WORK/message.bin"
    openssl pkeyutl -sign -rawin -inkey "$WORK/seed.pem" -in "$WORK/message.bin" -out "$WORK/domain.sig" 2>/dev/null
    cat "$WORK/region.bin" "$WORK/domain.sig" >"$WORK/manifest-openssl.bin"
    expect_status 0 "manifest signed independently by OpenSSL is accepted" "$TOOL" verify "$WORK/kernel.bin" "$WORK/manifest-openssl.bin" "$WORK/pub.key"
else
    echo "SKIP: OpenSSL with Ed25519 support not available; cross-check and domain-separation checks not run"
fi

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
