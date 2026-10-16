# Security

**This build is not production-secure.** The store restricts itself accordingly: the only installer is an unprivileged development backend, downloads work only from local files, and the sample trust store is a local file. `exile-store status` prints this posture.

## Enforced and tested

| Requirement | Mechanism | Test coverage |
|---|---|---|
| Verify signatures and publisher authenticity before install | Ed25519 over `(appId, version, package id, sha256, size)`; key must be in the trust store, not revoked, and authorised for the app's publisher | `test_trust.cpp`, `test_installer.cpp` |
| Reject invalid / tampered / untrusted packages | Digest + size check, then signature check; failures delete the download and install nothing | tamper, unsigned, wrong-key, swapped-package-and-digest tests |
| Replay of a genuine old signature as a new version | Version is inside the signed payload | `trust_signature_binds_version_...` |
| Never execute downloaded files | The store never runs anything it downloads; the dev backend only unpacks data files | n/a by construction |
| Malicious package URLs | Only `https://` and safe relative paths; `http://`, other schemes, `data:`, credentials-in-URL, `..`, absolute paths rejected at catalog load **and** again at fetch | `test_catalog.cpp`, `test_expkg_backend.cpp` |
| Path traversal / archive attacks | Package entry paths validated (no `..`, absolute, `\`, duplicates incl. case-insensitive, file/dir conflicts); entry/size limits; extraction stays under the destination; symlinks refused on pack and extract; relative sources resolved after symlink resolution and kept inside the catalog directory | `test_expkg_backend.cpp` |
| Dependency confusion | Dependencies are resolved only by exact `appId` from the one loaded catalog and the local library; there is no secondary index | `dependencies_are_checked_...` |
| Catalog metadata vs trusted signing info | Catalog carries signature *metadata*; keys live only in `trusted-keys.json` | by design; trust tests |
| Success only when really installed | Backend confirmation required before the library is updated | `success_is_claimed_only_after_the_backend_confirms` |
| No account needed, no tracking, no ads | None implemented; no network code at all in this build | n/a |
| Ratings/popularity not treated as safety | UI says a signature shows who published, not that an app is safe; no popularity sort | `test_ui.cpp` |
| Memory safety of parsers | Strict JSON and package parsers with depth/size limits; full suite clean under ASan + UBSan | CI-style run, see report |

## Local web service (`exile-store serve`)

| Threat | Mitigation | Test |
|---|---|---|
| Reachable from the network | Binds `127.0.0.1` only; no option to change this | `server_binds_loopback_...` |
| DNS rebinding | `Host` must be exactly `127.0.0.1:<port>` or `localhost:<port>` | `api_refuses_wrong_host_header` |
| Another local web page driving the store (CSRF) | Per-run 256-bit bearer token required on every `/api/*` call, constant-time compare; `Origin` and `Sec-Fetch-Site` must be same-origin; POST must be `application/json` | `api_requires_the_access_token`, `api_refuses_cross_origin_...`, `api_post_needs_json_...` |
| Token leakage | Printed once in a URL *fragment* (never sent in a request line or Referer), removed from the address bar on load, kept only in the tab's session storage; not accepted in query strings | `api_requires_the_access_token` |
| XSS from catalog text | Strict CSP (`default-src 'none'; script-src 'self'; style-src 'self'; ...`), no inline script/style/handlers, all data inserted with `textContent`, JSON served as JSON | `api_responses_carry_hardening_headers`, `api_app_text_is_data_...` |
| File disclosure / traversal | Only compiled-in assets are served; nothing is read from disk by path | `api_serves_only_embedded_assets_...` |
| Request smuggling, oversized or malformed input | One request per connection, no chunked bodies, duplicate Host/Content-Length refused, header/body caps, I/O timeouts, connection cap | `http_rejects_...`, `server_binds_loopback_serves_and_survives_garbage` |
| Web layer widening what installs are allowed | It only submits intents; the installer pipeline decides | `api_cannot_install_what_the_pipeline_refuses` |

Limits of this model: any process running as the same user can read the printed link (or the process's memory) and use the store as that user; the token only stops *other web pages and other machines*, not local malware. There is no TLS (loopback only). Remote access from another device is deliberately not offered. The library snapshot and installer are still the unprivileged development ones described below. Run under AddressSanitizer, UndefinedBehaviorSanitizer and ThreadSanitizer: the suite is clean.

## Known limitations (do not ship until addressed)

1. **No sandbox or permission enforcement.** Permissions are displayed and explained but nothing enforces them; that is the OS's job and the OS service is not here.
2. **No privileged installer.** The dev backend writes to a user prefix. A real installer must be a separate authorised service; the GUI must stay unprivileged.
3. **TOCTOU between verification and install.** The package sits in a user-writable cache between verification and unpacking. A local attacker with write access to the cache could swap it. The real installer service must take ownership of (copy or `fd`-pass) the verified bytes before unpacking, or re-verify.
4. **Catalog is not signed.** Package signatures stop an attacker who controls the catalog from installing *unsigned or untrusted* code. They do not stop that attacker from hiding apps, flipping a `withdrawn` flag, or **serving an older genuinely-signed (vulnerable) version for a fresh install**. Updates never downgrade, but a signed catalog with a freshness/expiry field and a minimum-version policy is needed.
5. **No HTTPS transport.** HTTPS URLs are accepted by the data model but refused at fetch with a clear message. TLS-capable transport (with certificate validation, redirect policy and size caps) must be plugged into `ITransport`.
6. **Trust store distribution.** The trust store is a plain local file. Production needs a protected, updatable root of trust (key rotation, revocation distribution, ideally a transparency log).
7. **Local `file://` sources are opt-in** (`--allow-file-urls`) and intended for development only.
8. **Library file is not tamper-protected.** It is a user-space record; the backend, not the library, is the source of truth for "installed".
9. **Secrets:** the sample generator creates a throw-away signing key per run and discards it. The repository contains no private keys.
