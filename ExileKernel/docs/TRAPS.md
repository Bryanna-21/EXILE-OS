# EUK x86-64 exception infrastructure (Mission C, batch 2)

Status: **implemented and host-tested; not yet validated in an emulator or on hardware.**
The emulator cases exist in `scripts/boot_test.py` but have not been executed. Until they pass,
loading the GDT/TSS/IDT and real exception delivery must be treated as unproven.

## Scope

The five roadmap tasks that follow Mission C batch 1, in the order of the Mission C
implementation list (Phase 1, then Phase 2):

| # | Task | Where |
|---|------|-------|
| 1 | Kernel stack configuration | `src/stacks.rs` |
| 2 | IDT loading | `src/traps.rs` (`build_idt`, `fill_idt`, `checked_pointer`, `init_bsp`) |
| 3 | Basic exception entry | `src/arch/x86_64/exceptions.S` (`euk_isr_common`), `src/kernel.rs` (`euk_trap_dispatch`) |
| 4 | Complete exception stubs | `src/arch/x86_64/exceptions.S` (256 stubs + `euk_isr_table`) |
| 5 | Register-frame representation | `src/frame.rs` |

`docs/ROADMAP.md` only lists coarse milestone gates, so the ordered task list is the one in the
Mission C brief. Task 1 includes installing the GDT and TSS (`lgdt`, segment reload, `ltr`),
which batch 1 deliberately deferred and which IST-based gates require.

## Control flow

```
CPU exception/interrupt
  -> IDT gate (interrupt gate, kernel CS, IST for #DF/NMI/#MC)
  -> stub for that vector (16 bytes): push $0 if the CPU pushed no error code; push $vector
  -> euk_isr_common: push 15 GPRs, cld, call euk_trap_dispatch(&mut TrapFrame)
  -> traps::handle: classify, write a report into a fixed buffer
  -> Resume: pop GPRs, drop vector+error code, iretq        Fatal: report to serial, halt
```

### Frame layout (`TrapFrame`, 176 bytes, lowest address first)

`r15 r14 r13 r12 r11 r10 r9 r8 rbp rdi rsi rdx rcx rbx rax | vector | error_code | rip cs rflags rsp ss`

Compile-time asserts pin every offset. The error code slot is always populated: a real hardware
code for vectors 8, 10, 11, 12, 13, 14, 17, 21, 29, 30 (the set in `idt.rs`), `0` otherwise.

### Stack alignment

In 64-bit mode the CPU aligns RSP to 16 bytes before pushing the 5-qword frame (6 with an error
code). The stub makes it 6 qwords in both cases; the vector plus 15 registers add 16 qwords. The
total of 22 qwords (176 bytes) keeps RSP 16-byte aligned at the `call`, as the SysV ABI requires.
Handlers must not use SSE/x87 (only general-purpose state is saved; the kernel target is built
without SSE).

### Stacks

`StackSet` holds four 16 KiB stacks in `.bss`: RSP0, IST1 (#DF), IST2 (NMI), IST3 (#MC).
`stacks::prepare` validates alignment, that each stack lies below the identity-mapped limit, and
that none overlap, before any TSS byte is written.

### Initialisation order (`traps::init_bsp`)

1. Fill the static IDT in place and verify every vector has a present gate (`checked_pointer`
   refuses otherwise). No CPU state has changed yet.
2. `stacks::install_bsp`: configure the TSS, `lgdt`, far-return to reload CS, reload data
   segments, `ltr`.
3. `lidt`.

Interrupts are never enabled here; no `sti` exists in this change.

## Fault policy

Only #BP resumes. Everything else is fatal: a report is written and the kernel halts. Page
faults are fatal because there is no virtual memory manager to resolve them. NMI and #MC are
fatal because their handlers are not re-entrancy safe. A trap taken while the handler is
running prints nothing further and halts (`enter_trap`).

## Verification matrix

| Check | Kind | Result |
|-------|------|--------|
| 100 unit tests (`python3 scripts/test.py`) | host unit tests | pass |
| Real `exceptions.S` stubs executed on the host CPU through `iretq` (all 256 vectors: frame fields, error-code normalisation, register preservation, DF handling, 16-byte alignment) | host execution of the real assembly | pass |
| Mutation checks of `exceptions.S` (swapped pops, missing `cld`, wrong error-code vectors, misaligned stack, wrong vector, retained error code) | test sensitivity | each mutation fails the suite |
| `#![no_std]` build of all modules, warnings as errors | compile-time | pass |
| `kernel.rs` type-check in 5 cfg combinations | compile-time | pass |
| Freestanding link of default and trap builds against the real linker script and `check.py` rules (host `core`, not `x86_64-unknown-none`) | link sanity | no undefined symbols; all `check.py` rules pass |
| Stub bytes and `euk_isr_table` inside the linked ELF | static | correct for all 256 vectors |
| `lgdt`/`retfq`/`ltr`/`lidt` as compiled | disassembly review | registers distinct, operands as intended |
| `scripts/build.py`, `scripts/boot_test.py` | bare-metal build, QEMU | **not run**: no `x86_64-unknown-none` target, QEMU or GRUB in the authoring environment, Rust there was 1.97 not the pinned 1.91.1 |

What the host execution does **not** show: that the CPU accepts the GDT/TSS/IDT, that exceptions
are delivered to these stubs, that IST stacks are used, or that the `--traps` boot succeeds.

## Emulator cases to run (scripts/boot_test.py)

* `traps`: GDT/TSS/IDT load, `int3` is handled and returns, then normal boot completes (`pc`, `q35`).
* `trap_ud`: `ud2` produces `EUK TRAP #UD` and a failing exit.
* `trap_df`: stack pointer set to unmapped memory then `int3`; delivery faults twice, and
  `EUK TRAP #DF` can only appear if the double fault runs on the IST1 stack.

## Limitations

* No guard pages: a stack overflow corrupts the neighbouring stack. Needs small-page mappings
  from the memory subsystem.
* Bootstrap CPU only; per-CPU GDT/TSS/IDT/stacks belong with per-CPU state and SMP bring-up.
* The IDT is not loaded in the default image; it is enabled with `build.py --traps`.
* No `swapgs` handling, no user-mode entry, no syscall path.
* A software `int N` for a vector that expects an error code would corrupt the frame. Those
  gates are DPL 0, so only ring 0 could do it.
* NMI/#MC share one IST stack each and are not nested-safe.
* No stack traces or symbol resolution; the report is registers, CR2 and decoded error bits.

## Dependencies before the next mission

* Run the three emulator cases above on a toolchain with Rust 1.91.1 and `x86_64-unknown-none`.
* Interrupt controller work (PIC/APIC) needs this IDT loaded and validated first.
* Page-fault recovery needs the Mission B virtual memory manager.
