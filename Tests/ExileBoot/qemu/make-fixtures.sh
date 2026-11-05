#!/usr/bin/env bash
# Deterministic boot-integrity fixtures.
#
# Usage: make-fixtures.sh /path/to/ExileBootManifest <output-dir>
#
# No key material is stored in the repository. The two test seeds are derived from fixed labels
# (SHA-256 of "exile-boot-test-fixture-A" and "-B"), so every run produces identical keys and, because
# Ed25519 signatures are deterministic, identical manifests. These keys are TEST KEYS ONLY: they are
# public by construction and must never be used as a real trust anchor.
#
# Output files (all in <output-dir>):
#   kernel.bin                 stand-in kernel payload (deterministic bytes, not a bootable kernel)
#   seedA, pubA, seedB, pubB   test key pairs
#   zero.key                   32 zero bytes (an invalid trust anchor)
#   manifest.valid             kernel.bin signed with seed A
#   manifest.modified          manifest.valid with one byte flipped
#   kernel.modified            kernel.bin with one byte flipped
#   manifest.wrong-signer      kernel.bin signed with seed B
#   EXPECTED.txt               scenario table: <scenario> <manifest> <kernel> <anchor> <expected verifier status>

set -eu
TOOL="${1:?usage: $0 /path/to/ExileBootManifest <output-dir>}"
OUT="${2:?usage: $0 /path/to/ExileBootManifest <output-dir>}"
mkdir -p "$OUT"

python3 -I - "$OUT" <<'PY'
import hashlib, sys
out = sys.argv[1]
# Deterministic stand-in kernel payload: 64 KiB derived from SHA-256 counter blocks.
data = b"".join(hashlib.sha256(b"exile-boot-test-kernel-" + str(i).encode()).digest() for i in range(2048))
open(f"{out}/kernel.bin", "wb").write(data)
for name in ("A", "B"):
    open(f"{out}/seed{name}", "wb").write(hashlib.sha256(("exile-boot-test-fixture-" + name).encode()).digest())
open(f"{out}/zero.key", "wb").write(bytes(32))
PY

# Derive the public keys from the seeds with OpenSSL (an implementation independent of the project's own Ed25519 code),
# then confirm with the project's tool (check-keys) that each seed really derives its public key.
python3 -I - "$OUT" <<'PY'
import subprocess, sys, os, tempfile
out = sys.argv[1]
for name in ("A", "B"):
    seed = open(f"{out}/seed{name}", "rb").read()
    pkcs8 = bytes.fromhex("302e020100300506032b657004220420") + seed
    with tempfile.TemporaryDirectory() as d:
        der = os.path.join(d, "k.der")
        open(der, "wb").write(pkcs8)
        pub = subprocess.run(["openssl", "pkey", "-inform", "DER", "-in", der, "-pubout", "-outform", "DER"],
                             check=True, capture_output=True).stdout
    # SubjectPublicKeyInfo for Ed25519 ends with the 32 raw key bytes.
    open(f"{out}/pub{name}", "wb").write(pub[-32:])
PY

"$TOOL" check-keys "$OUT/seedA" "$OUT/pubA" >/dev/null
"$TOOL" check-keys "$OUT/seedB" "$OUT/pubB" >/dev/null
"$TOOL" sign "$OUT/kernel.bin" "$OUT/seedA" "$OUT/manifest.valid" 1 --anchor "$OUT/pubA" >/dev/null
"$TOOL" sign "$OUT/kernel.bin" "$OUT/seedB" "$OUT/manifest.wrong-signer" 1 --anchor "$OUT/pubB" >/dev/null

python3 -I - "$OUT" <<'PY'
import sys
out = sys.argv[1]
m = bytearray(open(f"{out}/manifest.valid", "rb").read()); m[40] ^= 0xFF   # inside the kernel digest field
open(f"{out}/manifest.modified", "wb").write(m)
k = bytearray(open(f"{out}/kernel.bin", "rb").read()); k[30000] ^= 0xFF
open(f"{out}/kernel.modified", "wb").write(k)
PY

cat >"$OUT/EXPECTED.txt" <<'TABLE'
# scenario              manifest               kernel           anchor    expected
valid-accepted          manifest.valid         kernel.bin       pubA      Ok
modified-manifest       manifest.modified      kernel.bin       pubA      BadSignature
modified-payload        manifest.valid         kernel.modified  pubA      DigestMismatch
mismatched-seed         manifest.wrong-signer  kernel.bin       pubA      UnknownKey
invalid-anchor-zero     manifest.valid         kernel.bin       zero.key  refused-before-verification
untrusted-anchor        manifest.valid         kernel.bin       pubB      UnknownKey
TABLE
echo "fixtures written to $OUT"
