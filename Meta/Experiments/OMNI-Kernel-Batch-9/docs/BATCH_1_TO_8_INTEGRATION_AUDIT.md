# Batches 1 to 8 integration audit

Written from the source in the repository, not from earlier reports. Counts are raw declarations found by `grep`
(`test "` in Zig, `#[test]` in Rust); the numbers actually reported by test runners are in
`BATCH_9_VERIFICATION_REPORT.md`.

## Base

| Item | Value |
|---|---|
| Repository | `Bryanna-21/EXILE-OS` |
| Base (`origin/main`) | `686eecaf6598f34c84b06f783f508ab35f764398` |
| Batch 8 dependency | commit `4ab10276f316280eec5cf15a5b33688b1ce9b88e`, branch `experiment/omni-kernel-batch-8`, bundle SHA-256 `dbfd15538a95f5f7b2e00c323918a94e02ce63a01a145282728a529e9bb126e2` |
| Batch 8 merged into `main`? | **No.** `main` holds Batches 1 to 7. Batch 9 applies on `main` with the Batch 8 package present under `Meta/Experiments/OMNI-Kernel-Batch-8/`. |

## Per-batch record

Source commit ids: on `main` Batch 2 `45fbe98c`, 3 `3922a59a`, 4 `faa9273e`, 5 `d0b61526`, 6 `a11ed638`, 7 `aca63dc0` plus fix
`329da1cf`. Batch 8 is only on the local chain (`4ab10276`, built on Batch 7 `97c3b51f`). "Base commit" below is the parent the
batch was built on.

### Batch 1: OMNI-ABI specification
- Scope: prose specification (ABI 0.1 draft). Files: `Documentation/OMNI-Kernel/Batch-1/*` (5 files).
- Interfaces: none executable. Dependencies: none. Tests: none (no code).
- Verified: only that the Zig/Rust constants in later batches are consistent with each other (Batch 9 checker); not that
  they match the prose in every detail.
- Limitations: draft status. Integration: reference only.

### Batch 2: wire format, handles, rights (Zig, hosted)
- Source: `src/core/{capabilities,memory,resources,ipc}.zig`, `src/arch/*`, `src/tests.zig`; ~1.9 kLOC; 59 test declarations.
- Interfaces: `Handle`, `ObjectId`, `Rights`, `CapabilityTable`, `Account`, IPC `Header`/`encode`/`decode`, `UserWindow`, `Status` codes.
- Dependencies: Zig std only. Source of the status codes (-1..-17) reused by Batches 3 to 7.
- Limitations: models only; no hardware.
- Integration: its `Status` set is checked against Batch 5 and Batch 7 by `scripts/check_abi_conformance.py`. Not linked to any Rust code.

### Batch 3: capability lifecycle (Zig, hosted)
- Source: `src/{kernel,gen_arena}.zig` and a re-export of Batch 2; ~1.1 kLOC; 56 test declarations.
- Interfaces: `GenArena`, `ObjectIdAllocator`, revocation, per-space accounting.
- Limitations: models. Integration: consumed by Batch 4 and 5 as Zig modules only.

### Batch 4: IPC core (Zig, hosted)
- Source: `src/ipc.zig`, `src/batch2.zig`, tests; ~1.3 kLOC; 63 test declarations.
- Interfaces: channels, queues, capability transfer, `OpToken`, `Desc`.
- Limitations: models; no blocking (Batch 5 adds it). Integration: Zig only.

### Batch 5: kernel IPC and proposed syscall boundary (Zig, hosted)
- Source: `kernel_ipc.zig`, `syscall.zig`, `accounting.zig`, `wait_queue.zig`, `hosted_user_memory.zig`; ~1.1 kLOC; 56 test declarations.
- Interfaces: `KernelIpc`, `Nr` (IPC syscalls 0x100 to 0x107), `status_fault = -32`, `status_no_sys = -33`, register convention
  (rax number; rdi, rsi, rdx, r10, r8, r9 arguments).
- Limitations: hosted `UserMemory`; no real address space.
- Integration: its numbers and status codes are cross-checked; the IPC syscalls are **not** implemented by the Rust kernel
  (Batch 7 returns `NO_SYS` for them; tested by `sys_01`).

### Batch 6: architecture and scheduler contracts (Zig, SIM)
- Source: `sched.zig`, `thread_state.zig`, `ipc_sched.zig`, `entry.zig`, `context.zig`, `user_memory.zig`, `arch.zig`; ~1.6 kLOC; 53 test declarations.
- Interfaces: `Sched` (16 threads), thread state machine, `IpcSched`, `SimCpu`, `Registry` of user-memory models.
- Limitations: SIM only; the "CPU" is a model. Integration: contracts that Batch 7 re-implements in Rust; no shared code.

Zig total by raw declaration: 59 + 56 + 63 + 56 + 53 = 287. Reported results by the Zig runner differ slightly (see
verification report); the difference is not hidden there.

### Batch 7: cooperative scheduler and syscall entry (Rust, `no_std`, x86-64)
- Source: `kernel/{sched,syscall,uaccess,msr,context,sync,status,lib}.rs`, `arch/x86_64/{context,syscall_entry}.S`; ~3.2 kLOC;
  56 test declarations. Hosted tests execute the real context-switch assembly.
- Interfaces: `Sched`, `Kernel::syscall`, `Spaces`, `UserBacking`, `USER_TOP`, `msr::*`, `validate_frame`, `entry_stub_bytes`, `SpinLock`.
  Syscalls 0x1F0 to 0x1F4; debug 0x1F5 to 0x1F7 off by default.
- Limitations: cooperative; uniprocessor; 16 threads; 16-entry exit log; one syscall stack; `UserBacking` is a trait with only
  a test implementation, i.e. no real page-table check.
- Integration: Batch 8 and Batch 9 depend on it as a path dependency, unchanged.

### Batch 8: bare-metal image (Rust, `no_std`, x86-64)
- Source: `src/{desc,frames,idt,multiboot,paging,report,selftest,timer,usermode}.rs`, `kernel/main.rs`, `arch/x86_64/{boot,isr,probe}.S`,
  `link/linker.ld`, `scripts/*`; ~3.5 kLOC; 65 test declarations.
- Interfaces: GDT/TSS (`desc`), IDT/trap frame (`idt`, 176-byte frame), W^X `Mapper`, `translate`, `FrameAllocator`, `iret_frame`, self-tests.
- Limitations: **never booted.** QEMU tests are written and report NOT RUN. Host-target fallback toolchain (rustc 1.97.0), not the pinned
  1.91.1 with `x86_64-unknown-none`. Mapper can leak intermediate tables on partial failure. Image mixes interrupt masking
  (see Security review SEC-05).
- Integration: its `Mapper`/`translate` was not connected to Batch 7's `UserBacking`. **Batch 9 supplies that connection.**

## What was missing between the batches (and what Batch 9 does about it)

| Gap | Batch 9 response |
|---|---|
| Batch 7 `UserBacking` was never backed by Batch 8 page tables | `PagedBacking` (`src/lib.rs`), tested with real `Mapper`-built tables |
| No test exercised Batch 7 and Batch 8 together | 34 hosted end-to-end and conformance tests |
| Constants duplicated across Zig and Rust with no check | `scripts/check_abi_conformance.py` (23 checks, 7 self-test mutations) |
| IPC/capabilities (Zig) not wired into the Rust kernel | **Not done.** Documented as NOT IMPLEMENTED; see Support matrix |
| Batch 8 never booted | **Not done.** Cannot be done without QEMU or hardware |

## Findings recorded during the audit

1. IPC and capabilities exist only in Zig models; the Rust kernel has no IPC.
2. The Batch 8 fallback image does not mask interrupts inside `SpinLock` (`irq_save`/`irq_restore` are `cfg(target_os = "none")` only in Batch 7).
3. `Mapper` partial failure leaves allocated table frames behind (documented by test `mem_16`).
4. Exit log keeps only the last 16 exits (test `sys_08`).
5. One per-CPU syscall stack.
6. No ABI version query in the Rust syscalls.
7. The syscall register convention in `Kernel/` (rax, rdx, rdi, rbx, rsi) differs from Batch 5/7 and is unresolved.
8. GDT selector constants are duplicated in Batch 7 and Batch 8 (checked identical by `cf_01`).
9. Package versions are inconsistent (`0.0.0` in Batches 7 and 8).
10. rustc 1.97.0 prints `unused_unsafe` warnings that 1.91.1 would not; one warning remains in the Batch 8 build.

## Cleanup review (spec section 10)

Resolved in Batch 9 (additive only):

- Missing integration between Batch 7 `UserBacking` and Batch 8 paging: `PagedBacking`.
- Unchecked duplication of constants across languages and batches: `scripts/check_abi_conformance.py` and `cf_01`..`cf_05`.

Identified and deliberately **not** changed, because the fix lies in Batch 1 to 8 files or protected paths (no breaking change was made;
proposals are in `PROPOSALS.md`):

| Item | Category | Reason not changed |
|---|---|---|
| GDT selector constants duplicated in B7 and B8 | duplicate definitions | Changing either batch rewrites history; identity is now tested (`cf_01`) |
| Status codes repeated in B2, B5, B7 | inconsistent error types (none found) | Identical, checked by the script |
| Syscall register convention differs from `Kernel/` | incompatible ABI assumption | Owner decision (P-04) |
| Two scheduler implementations (Zig B6 model, Rust B7) | redundant abstraction | Intentional: model versus implementation; no shared code |
| Package versions `0.0.0` in B7, B8 | inconsistent versioning | P-08 |
| Batch 8 `Mapper` partial-failure leak | incorrect cleanup | P-02 |
| Batch docs refer to earlier locations | incorrect documentation | Historical evidence kept intact |

No historical batch evidence was deleted, and no earlier test was removed or altered.
