#!/bin/sh
# Packaging/integration smoke test: builds a signed dev catalog with the shipped
# tool, then drives the real exile-store binary through install -> update ->
# uninstall, and checks the refusal paths. Usage: smoke_test.sh <exile-store> <expkg-tool> <workdir>
set -eu

STORE=$1
TOOL=$2
WORK=$3

rm -rf "$WORK"
mkdir -p "$WORK"
"$TOOL" devcatalog "$WORK/data" >/dev/null
CAT="$WORK/data/catalog/catalog.json"
UPD="$WORK/data/catalog/catalog-update.json"
STATE="$WORK/state"

run() { "$STORE" --catalog "$CAT" --data-dir "$STATE" "$@"; }
run_upd() { "$STORE" --catalog "$UPD" --data-dir "$STATE" "$@"; }
fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

# status reports the real verifier and an honest security posture
run status | grep -q "OpenSSL Ed25519" || fail "verifier not reported"
run status | grep -q "NOT production-secure" || fail "security posture missing"

# install a trusted, compatible app
run install org.exile.sample.notes | grep -q "was installed" || fail "install did not report success"
[ -f "$STATE/prefix/apps/org.exile.sample.notes/current/README.txt" ] || fail "payload not on disk"
run library | grep -q "Sample Notes  1.0.0" || fail "library does not list the app"

# refusals must exit non-zero and leave nothing behind
if run install org.exile.sample.untrusted >/dev/null 2>&1; then fail "untrusted package was installed"; fi
[ ! -d "$STATE/prefix/apps/org.exile.sample.untrusted" ] || fail "untrusted payload exists"
if run install org.exile.sample.pocketclock >/dev/null 2>&1; then fail "incompatible app was installed"; fi
if run install org.exile.sample.player >/dev/null 2>&1; then fail "dependency check bypassed"; fi
if run install org.exile.sample.futureos >/dev/null 2>&1; then fail "OS-too-old app was installed"; fi
if run install org.exile.sample.betaonly >/dev/null 2>&1; then fail "beta app installed without opt-in"; fi

# update flow
run_upd library | grep -q "update to 1.1.0" || fail "update not detected"
run_upd update --all | grep -q "was updated" || fail "update failed"
run library | grep -q "Sample Notes  1.1.0" || fail "library not updated"

# dependency-protected uninstall
run install org.exile.sample.codecs >/dev/null || fail "codecs install"
run install org.exile.sample.player >/dev/null || fail "player install with dependency"
if run uninstall org.exile.sample.codecs >/dev/null 2>&1; then fail "dependency removed under a dependent"; fi
run uninstall org.exile.sample.player >/dev/null || fail "player uninstall"
run uninstall org.exile.sample.codecs >/dev/null || fail "codecs uninstall"
run uninstall org.exile.sample.notes >/dev/null || fail "notes uninstall"
[ ! -d "$STATE/prefix/apps/org.exile.sample.notes" ] || fail "files left after uninstall"

# offline: with the catalog gone, the cached copy is used and the notice says so
mv "$CAT" "$CAT.gone"
run home | grep -q "saved catalog data" || fail "offline cache notice missing"
mv "$CAT.gone" "$CAT"

# web service: loopback, token required, real install through the API
if "$STORE" --help 2>&1 | grep -q "serve \[port\]" && command -v curl >/dev/null 2>&1; then
    SLOG="$WORK/serve.log"
    "$STORE" --catalog "$CAT" --data-dir "$WORK/state-web" serve 0 >"$SLOG" 2>&1 &
    SPID=$!
    i=0; while [ $i -lt 50 ] && ! grep -q "#t=" "$SLOG" 2>/dev/null; do sleep 0.1; i=$((i+1)); done
    URL=$(grep -o 'http://127.0.0.1:[0-9]*' "$SLOG" | head -1)
    TOK=$(grep -o '#t=[0-9a-f]*' "$SLOG" | head -1 | cut -c4-)
    [ -n "$URL" ] && [ -n "$TOK" ] || { kill $SPID 2>/dev/null; fail "serve did not start"; }
    [ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/session")" = 401 ] || { kill $SPID; fail "api open without token"; }
    [ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/")" = 200 ] || { kill $SPID; fail "ui not served"; }
    curl -s -X POST -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" -d '{"appId":"org.exile.sample.notes"}' "$URL/api/install" >/dev/null
    i=0; while [ $i -lt 100 ]; do
        curl -s -H "Authorization: Bearer $TOK" "$URL/api/library" | grep -q "org.exile.sample.notes" && break
        sleep 0.1; i=$((i+1))
    done
    kill $SPID 2>/dev/null; wait $SPID 2>/dev/null || true
    [ -d "$WORK/state-web/prefix/apps/org.exile.sample.notes" ] || fail "web install did not land"
fi

echo "smoke test passed"
