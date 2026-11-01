# Non-kernel system architecture

Status: proposal for review, grounded in `Documentation/Project-Audits/NON_KERNEL_SYSTEM_AUDIT.md`. Nothing here claims that a component exists unless the audit says so.

## Principles

1. **Userland never stands in for a kernel primitive.** Isolation, memory protection, driver access, and monotonic counters are kernel or firmware features. Where a layer needs one that is absent, the layer documents the dependency instead of simulating it.
2. **Reuse what exists.** WindowServer, LaunchServer, SystemServer, ConfigServer, NotificationServer, LibFileSystem (Trash), the Store's core library and LibCrypto are the building blocks. New components are added only where the audit found nothing.
3. **Fail closed on trust.** Anything that decides whether code runs (boot manifest, package, update) rejects on every doubt and reports a specific reason.
4. **One statement of status.** Each subsystem has a document that states what is and is not verified.

## Layers

```
Applications        XFiles  Bridgeline  SoundPlayer  Settings  ExileStore ...
Desktop             WindowServer  Taskbar  LaunchServer  NotificationServer
System services     SystemServer(init)  ConfigServer  LoginServer  [log] [health] [time] [update]
Trust               Store trust (packages)        Boot trust (kernel image)
Kernel interfaces   syscalls, IPC, devices      (out of scope: Kernel/, ExileKernel/)
Firmware/boot       EFI prekernel + signed boot manifest (opt-in)
```
Items in `[brackets]` do not exist yet.

## Trust model

* **Boot trust** (exists, opt-in): a public key compiled into the EFI prekernel authenticates a signed manifest binding the embedded kernel image (size and SHA-256). It is only as strong as the platform's authentication of the prekernel itself (UEFI Secure Boot). Scope and limits: `Documentation/BootIntegrity.md`.
* **Package trust** (exists, development only): Ed25519 signatures checked against a local trust file by the Store (`Userland/Applications/ExileStore/docs/SECURITY.md`).
* **Bridge (not designed)**: a future update service must chain package and image trust to a root that the boot anchor can authenticate, with freshness and rollback protection. Until that is designed, the two roots stay separate and nothing may claim an end-to-end chain.
* **Key separation**: production signing seeds stay offline and outside the source tree; tests generate throw-away keys in scratch directories and never use them as real anchors.

## Service contracts to define (Phase Two)

| Service | Existing base | Contract to document |
|---|---|---|
| Init | SystemServer, `Base/etc/SystemServer.ini` | start modes, dependencies, restart policy, failure escalation |
| Logging | none | IPC for structured log submission, retention, access control |
| Health | none | what is monitored, how failures are surfaced |
| Time | `ntpquery` | clock-set permission, source configuration |
| Config | ConfigServer | per-user and system scopes |
| Sessions | LoginServer | session lifecycle, who may start one |

## Dependencies on the kernel team

Listed in section 5 of the audit. The most important: sandbox enforcement for application permissions, a monotonic counter for rollback protection, storage-health interfaces.
