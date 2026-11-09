# Userland integration notes

Read from the repository; nothing was run.

- **Syscall convention.** `Kernel/API/Syscall.h` invokes `syscall` on x86-64 with the number in rax and arguments in rdx, rdi, rbx, rsi; on
  AArch64 it uses `svc #0` with the number in x8; on RISC-V `ecall` with a7. Userland that targets OMNI must either use `AbiMode::Exile`
  (Batch 10 maps rdx, rdi, rbx, rsi to OMNI's arguments) or the OMNI convention (rdi, rsi, rdx, r10, r8, r9).
- **IPC today.** `Userland/Libraries/LibIPC` and `Userland/Services` (SystemServer, WindowServer, and others) communicate over local sockets. They do not
  use OMNI channels. Converting them is migration step 6 and needs the `Kernel/` port first.
- **Capabilities versus pledge/unveil.** `Kernel/` offers `pledge` and `unveil` (promise-based). OMNI capabilities are an additional model, not a
  replacement. A userland library would hold handles instead of file paths.
- **Using the OMNI IPC from a test program** (hosted): see `tests/ipc_e2e.rs` for the exact byte layouts of each call.
- **No userland file was changed.**
