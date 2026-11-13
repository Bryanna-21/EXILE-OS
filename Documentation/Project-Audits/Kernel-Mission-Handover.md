# Kernel mission handover (2026-11-13)

Scope: the "complete kernel engineering" brief. Environment: no cross toolchain, no QEMU, no xorriso, no network.
Consequence: no kernel build, no boot, no QEMU runtime level (Levels 3-6) was executed. Nothing below claims otherwise.

## Done and verified (host only)
| Item | Evidence |
|---|---|
| `AK/Time.h`: `seconds_since_epoch_to_year` was `constexpr` but returns through non-literal `Tuple<>`; Clang 18.1.3 rejects it under `-Werror` (-Winvalid-constexpr). Now `inline`; all callers are runtime. | Lagom host build with Clang 18.1.3, no warning suppression: AK, LibCoreMinimal, IPCCompiler, IPCMagicLinter build. 37,634 timestamps from year 1 to 9999 compared with Python datetime: 0 mismatches. |
| Boot guardian (`Kernel/Boot/Exile`): SHA-256 and verification decision logic. | `Tests/ExileBoot/host-guardian-check.sh`: 301 message sizes (0-300, covering both padding paths) match an independent SHA-256; 12 verification cases pass (valid, no anchor, truncated, over-long, missing, null verifier, bad signature, unknown key, bad version, non-zero flags, tampered kernel, size mismatch). Uses a fake signature verifier: Ed25519 itself is NOT exercised here. |

## Reviewed, no defect found
`Kernel/Boot/Exile/BootGuardian.cpp` and `BootManifest.h`: exact-length parsing, fail-closed on missing anchors,
signature checked before any manifest field is trusted, constant-time compares, domain-separated signing message.

## Not done (blocked)
Everything requiring a kernel build or boot: virtual memory, interrupts/timers, SMP, scheduler, processes,
syscalls, IPC and capabilities in the real kernel, drivers, storage, networking, power. No code was written for
these because it could not be built or run here; unverified kernel code would overstate progress.
Earlier experimental OMNI batches (hosted models) remain Experimental, not integrated kernel functionality.

## Reproduce on a Linux host with the toolchain
    Meta/serenity.sh run x86_64     # full build and QEMU boot (not run here)
    Tests/ExileBoot/host-guardian-check.sh   # run here, passes

## Blocker register
1. Cross toolchain + QEMU + ISO tooling absent here (all of Levels 3-6).
2. Clang 17-19 claim confirmed only for AK/LibCoreMinimal/IPC tools, not the whole Lagom tree.
3. Ed25519 path of the boot guardian needs the LibCrypto-based `TestBootGuardian` run (needs the Lagom test setup).
