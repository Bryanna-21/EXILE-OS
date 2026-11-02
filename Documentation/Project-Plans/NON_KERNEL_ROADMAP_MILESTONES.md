# Non-kernel roadmap: measurable milestones

Extends `NON_KERNEL_ROADMAP.md` (phases) with milestones and acceptance criteria. A milestone is done only when **every** criterion is observed and the evidence is recorded in a report. No dates are given.

| ID | Milestone | Acceptance criteria (all required) | Depends on |
|---|---|---|---|
| M0 | Host build green | `cmake -S Meta/Lagom -B Build/lagom && cmake --build Build/lagom` succeeds with the project's supported compiler; `ctest` runs; the host CI job is green on `main` | Fix of the two `LibCore` errors (addendum, finding 1) |
| M1 | Boot-trust tests in CI | `ctest -R "ExileBoot\|TestBootGuardian"` shows `TestBootGuardian` (12 cases) and `ExileBootTamperTests` (53 checks) passing in the CI log | M0 |
| M1b | Anchor enforced in the build | A configure with a mismatched seed and anchor fails before `EFIPrekernel` compiles; a seed inside the source tree fails at configure; both observed in the real build | M0, review of `exile_boot_verification_checks.cmake` |
| M2 | Prekernel verification boots | `Kernel.efi` built with `ENABLE_BOOT_VERIFICATION=ON` boots to the kernel under QEMU+OVMF and prints the "verified" message; `run-qemu-boot-tests.sh` reports 3 of 3 passing; serial logs archived | target toolchain; M1b |
| M2b | Secure Boot chain documented and tested | A signed `Kernel.efi` boots under OVMF with Secure Boot enforcing, and an unsigned or altered one is refused by firmware; the trust-anchor documentation states exactly what is covered | M2 |
| M3 | Init contract | `SystemServer` start modes, dependencies and restart policy documented; tests cover restart limit, dependency failure and mode selection; run in CI | M0 |
| M4 | Log, health and time services | Each has an `.ipc` contract, failure-path tests and a CI run; the time service sets the clock from a configured source only with the right permission | M3 |
| M5 | App duplication resolved | One file manager and one browser application built; removed or renamed sources listed in a decision record; attribution preserved | product decision |
| M6 | Signed package and update design | A written design, reviewed, covering the chain from the boot anchor to packages, freshness, rollback and recovery; the Store's nine limits each mapped to a work item | M2 |
| M7 | Installer and recovery | A VM boots an installer from release media, installs, reboots, and a recovery entry restores a deliberately broken install; reproducible-build check (two builds, equal hashes) recorded | M3, M4, M6 |
