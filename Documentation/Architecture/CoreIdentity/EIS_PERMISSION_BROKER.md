# EIS intelligent system and the permission broker (Priority 3)

"EIS" is not defined in the repository. This is a proposal; **only the broker is implemented** (`Userland/Libraries/LibExilePolicy`, 38 host tests, mutation-checked, sanitizer-clean). No model runtime, provider adapter, or agent code exists or is claimed.

## 1. Architecture (Proposed)

```
 user ── approvals/revocations ──▶  Broker  ◀── Request{principal, permission, resource} ──  EIS agent process (D3)
                                      │  decisions + hash-chained audit                         ▲ only AgentFacade::request
                                      ▼                                                         │
                         services (D2) ask the broker before acting for a caller   ◀── model (local first; cloud optional)
```

* **Local first.** The default model runs locally as an ordinary D3 process with *no* `inet` promise and no `net.connect` grant. A cloud provider is an optional adapter that needs `ai.cloud:<host>:443` (High risk, so never standing-and-broad for an agent) — data leaves the machine only through that one permission, which is visible and revocable.
* **The model is not a trust anchor.** It produces text and tool-call *requests*. Every tool call is a broker request made by the agent's principal; the model has no other path to the filesystem, processes, network or settings. A hallucinating, jailbroken or prompt-injected model therefore can do at most what its principal was granted.
* **The broker is independent of the model:** it contains no model code, no prompts, no heuristics about content, and decides purely from principal, permission, resource and grants (T).

## 2. Authorization model (Built)

| Domain | Permission(s) | Scope | Risk |
|---|---|---|---|
| Filesystem | `fs.read`, `fs.write` | normalised absolute path or `$root` | medium / high |
| Process | `proc.spawn`, `proc.background` | one program path / none | high / medium |
| Network | `net.connect`, `ai.cloud` | `host:port` (`*.suffix` allowed, flagged broad) | medium / high |
| System | `sys.settings`, `sys.install` | settings key / none | **critical** |
| Devices / data / UI | `dev.*`, `data.contacts`, `ui.*` | none | low–high |

Decision order (T): unknown principal → deny; suspended → deny; malformed/traversal → deny; not in the **declared ceiling** → deny; matching live grant → allow (Once consumed); Low risk and declared → allow; else **needs approval** (deduplicated, ≤32 pending per principal); everything else — including any internal error — deny. Default is deny.

Hard rules (T): Critical permissions can only be granted **Once**. An agent can never hold a broad-scope grant except Once, never a Standing High-risk grant, and Until-grants for agents are capped at 24 h. A grant needs a named authorising user. Grants cannot exceed the declared ceiling even with user approval.

## 3. Approval, revocation, audit

* **Approval:** a request that needs approval creates a pending item; `resolveApproval` (user path) creates a grant for the *exact* requested resource unless the user chooses otherwise; lifetime Once / Session / Until / Standing within the rules above.
* **Revocation:** `revokeGrant`, `revokeAll`, `suspend`, `updateDeclared`, `onProcessExit`. Effective for the next request immediately (T). **Limit:** authority already projected into a running process as pledge/unveil cannot be loosened by the kernel; `RevokeResult.needsProcessRestart` says so, and the launcher must restart the process (**not implemented**).
* **Audit:** every request, decision, approval, grant, revocation, suspension and exit is appended to a chain where each entry's SHA-256 covers the previous hash. `verifyAuditText` detects edited, deleted, reordered entries and a forger who recomputes an edited entry's own hash (T). Hostile text (newlines, `|`, control characters, 5 KB justifications) is escaped/truncated (T). **Limits:** SHA-256 is unkeyed, so an attacker who can rewrite the *whole* log can forge a consistent one, and tail truncation is undetectable from the text alone; `auditHead()` must be anchored off-host (**not implemented**). The log is in memory.

## 4. "An AI failure cannot bypass operating-system permissions"

What is true today, and by what evidence:

| Claim | Basis |
|---|---|
| The object handed to an agent cannot grant/approve/revoke/suspend | Compile-time `static_assert`s (T) |
| Path-traversal, sibling-prefix and wildcard tricks do not widen a grant | Unit tests incl. an injection-style test (T) |
| Broker errors deny | Tests + `failClosedAllows` (T) |
| An agent *process* cannot call the broker's authority methods | **Not established**: needs process separation and an authenticated IPC channel, i.e. a booted kernel (N) |
| The kernel stops the agent doing what the broker did not grant | **Not established**: pledge/unveil projection is specified and checked against the real promise list (T) but never applied to a live process (N) |

So the guarantee is currently *at the policy layer*. Treat the system-level guarantee as unproven until the boot-validation milestone and a launcher exist.

## 5. Not built

Model hosting, provider adapters, prompt/tool plumbing, rate limiting beyond the pending cap, durable audit storage, user-facing approval UI, a broker daemon/IPC service.
