# EUK_INTEGRATION_PLAN.md (PROPOSED DRAFT)

Goal: an independently implemented Exile Universal Kernel (EUK), hybrid architecture, x86-64 and ARM64 first, developed
alongside the current kernel and swapped in gradually. Starting point: the current kernel is in-kernel monolithic
(drivers, filesystems, network stack under `Kernel/`), inherited from SerenityOS.

## Principles
1. **Clean separation**: EUK lives in its own tree (for example `EUK/`), not inside `Kernel/`, with its own license file and per-file headers. Nothing is copied from `Kernel/`.
2. **Provenance discipline**: keep a written record of who wrote what, from which sources. Contributors who have studied the current kernel are not "clean room"; decide the rule and have counsel review it.
3. **Keep the current kernel shippable** until EUK passes the gates below.
4. **No claim of independence** for the whole repository until the code actually is.

## Phases and gates

| Phase | Deliverable | Gate to continue |
|---|---|---|
| 0. Baseline | Green CI on current kernel for x86-64 and ARM64 with recorded QEMU boot | CI green, boot logs archived |
| 1. Contract | Written EUK design: hybrid split (what stays in kernel: scheduling, memory, IPC, interrupts; what moves to isolated user-space servers: filesystems, network, most drivers), capability model, IPC spec | Design review; decision on syscall ABI (keep POSIX-like ABI for userland reuse, or define a new one plus a shim) |
| 2. Bring-up | EUK boots on QEMU `virt` (ARM64) and q35 (x86-64): serial output, MMU, interrupts, timer, scheduler, IPC | Boot test in CI per arch |
| 3. Boot chain | Reuse EFI prekernel approach or write a new loader; implement **real** manifest verification (signed, fail closed). Existing `Kernel/Boot/Exile` SHA-256 is correct but the verifier is a stub | Tamper test: modified kernel refuses to boot |
| 4. Userspace bridge | Syscall/ABI compatibility layer so existing LibC and core services run on EUK | Existing `Tests/LibC` and `Tests/Kernel` equivalents pass |
| 5. Services | Move storage, network, input, graphics drivers behind IPC one at a time, each with fault-injection test (crash a driver, system survives) | Per-service pass, performance budget agreed |
| 6. Dual-boot selection | Image carries both kernels; recovery boots the old one; A/B update with rollback | Update/rollback test |
| 7. Switch default | EUK becomes default once it passes the same test suite as the old kernel; old kernel kept one release as fallback | Soak period, no regressions |

## Validation
- Same emulator test harness as current CI (`system_mode=self-test`, "Failed: 0" check).
- Fuzzing of IPC and syscall boundaries; sanitizer builds.
- Per-architecture matrix: nothing is called supported without a recorded emulator boot, and hardware support needs named boards.

## Risks
- Reusing current Userland/LibC means ABI choices bind EUK early.
- Time: device drivers dominate effort; plan target hardware list first.
- Legal: independent-authorship claims need qualified review (see licensing audit).
