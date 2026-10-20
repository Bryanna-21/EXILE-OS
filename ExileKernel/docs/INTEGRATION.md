# Existing Exile OS integration boundary

Audited snapshot: Bryanna-21/EXILE-OS main, 72c30495e8985d2dc6bf9b04cd0cb87617c77487; GitHub main resolved before and after archive download. Source archive SHA256: 7a82a79fc53b8d1d9f7e215ae1c3f1eb5203561bf8072fb13ea2412224a68aa5.

Existing `Kernel/Prekernel/multiboot.S`, `Kernel/Prekernel/linker.ld`, `Kernel/CMakeLists.txt`, `Meta/grub-gpt.cfg` and `Meta/build-image-grub.sh` use a Serenity-derived prekernel/main-kernel handoff. This is not EUK's ABI. No implementation was reused. EUK has its own Multiboot assembly, ELF, GDT, page tables and standalone GRUB test image. Do not overwrite `/boot/Kernel` or register EUK as the default OS kernel.

The snapshot Store CMake target now uses `exile_app` and `src/native_main.cpp`, links native GUI/Core/Gfx/Main libraries, and retains a separate `CMakeLists.host.cmake`. No Store files are changed or distributed in this package. This observation is not a verified integrated OS build. No prior installer fix is reapplied to the snapshot.

Before existing userland can run: define compatibility for LibC/syscalls, ELF userspace loader, signals, process/thread semantics, virtual memory, file descriptors, filesystem and device services, IPC and permissions. Then implement and test storage, input, graphics, networking, hardware discovery and package-service requirements. None exists in this milestone.

Migration gates: validate current OS baseline independently; kernel ABI design review; exception/memory protection and user-mode isolation tests; per-driver real functionality and failure tests; equivalent Tests/Kernel and Tests/LibC regressions; dual-boot recovery and update/rollback tests; multi-architecture boot evidence; explicit owner acceptance. Only then change boot selection, and never silently switch defaults.

Repository safety: downloaded a fresh read-only reference archive rather than touching a user checkout. No Git reset/clean/checkout/commit/push/stash or process termination was performed. No full OS build was active in the inspected process list. No claim is made about the user's remote working tree or stashes; an archive has neither. Existing OS/kernel/Store remain outside this deliverable.
