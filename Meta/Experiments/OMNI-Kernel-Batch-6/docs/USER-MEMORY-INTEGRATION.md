# User-memory integration

Status: a hosted page model (`src/user_memory.zig`) behind the unmodified Batch 5 `UserMemory`. No MMU, page table or
fault handler is involved.

## Boundary

Batch 5 sees only `UserMemory{ ctx, copy_in, copy_out, probe_write }`. Hosted tests use a flat segment model; a real
kernel supplies functions backed by the calling address space. Batch 6 shows the contract with `PageModel` + `Binding`:

* **No pointer is ever trusted or dereferenced.** User addresses are numbers; only the copy functions touch memory.
* **Range check first**, with overflow-safe arithmetic: `addr + len` must not wrap and must not exceed the user limit
  (`0x0000_8000_0000_0000` x86-64, `0x0001_0000_0000_0000` ARM64 48-bit). Kernel-half addresses are refused (UM-02).
* **Per page**: present, and readable (copy_in) or writable (copy_out, probe). A copy may span pages (UM-06).
* **copy_in** writes only a kernel buffer: a fault has no user-visible effect.
* **probe_write** checks without writing; Batch 5 probes the result and buffer BEFORE consuming a message (SYS-08).
* **copy_out** probes all pages first (UM-03). A fault found after the probe (another thread unmapped a page) can leave a
  PARTIAL write. It is reported as `Fault`, counted, and the written range is unspecified (UM-04). Batch 5 then releases
  the capabilities it delivered and drops the message (UM-08).
* **Zero length** always succeeds and touches nothing.
* **Ownership**: a `Binding` is created for one calling space and refuses a page model owned by another
  (`WrongAddressSpace`); every operation re-checks (UM-05). The registry maps spaces to models; the lookup key comes from
  the thread's owner (entry tests ENT-02, ENT-03).

## Requirements for the real implementation

1. fault-safe copy primitives like the existing `safe_memcpy`/`copy_from_user` (guarded instruction ranges with a fixup
   address). NOTE: they may stop part-way and report the fault address; a caller needing all-or-nothing must pre-validate,
   as `PageModel` does, and accept the unmap race for copy_out;
2. SMAP (x86-64) / PAN (ARM64) enabled except inside those primitives;
3. page-fault handling inside a guarded copy: resolve demand faults, otherwise return `Fault`; never panic and never
   fault-in from a context that cannot sleep;
4. permissions from the VMAs/page tables of the CALLING thread's address space, read under that space's lock or with a
   defined race (above);
5. a copy must not hold a spinlock across a possible page fault.

## Differences from the Batch 5 hosted model

Batch 5's `HostedUserMemory` never spans mappings and is therefore stricter than a real kernel; Batch 6's model spans
pages. Batch 5 is unchanged and its behaviour does not depend on the difference.

## Status

implemented (hosted): `PageModel`, `Binding`, `Registry`, fault and race injection. specified: everything in "Requirements".
deferred: demand paging, COW, shared memory, huge pages, TLB shootdown on unmap.
