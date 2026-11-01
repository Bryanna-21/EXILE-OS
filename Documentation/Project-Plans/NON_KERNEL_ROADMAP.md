# Non-kernel roadmap

Sequence follows `Documentation/Project-Audits/NON_KERNEL_SYSTEM_AUDIT.md` section 8. "Exit criteria" are what must be demonstrated before a phase is called done. No date estimates are made.

| Phase | Scope | Exit criteria | Blocked by |
|---|---|---|---|
| 0 Audit | Inventory with evidence | Audit document reviewed | none (done) |
| 1 Boot and trust | Signing-tool validation, tamper tests in CTest | 44 tamper checks and 12 verifier cases pass; tests registered in CTest and run by the host CI job | none for the tool side |
| 1b Boot on target | Apply the prekernel build patch; build with `ENABLE_BOOT_VERIFICATION=ON`; QEMU boot of a valid image and refusal of a tampered one | Both outcomes observed in QEMU and logged | target toolchain; kernel-owner review of a `Kernel/` build change |
| 2 Core services | SystemServer contract and tests; logging; health; time | Each service has a documented IPC contract, failure-path tests and a CI run | none |
| 3 Storage and network | Mount and storage monitoring on existing kernel interfaces; network manager, DNS, diagnostics | Same as above; Wi-Fi explicitly deferred | Wi-Fi: drivers |
| 4 Desktop | Decide duplicate apps (XFiles/FileManager, unbuilt Browser copy); shell, notifications, accessibility gaps | Product decision recorded; one app per role | product decision |
| 5 Applications | Archive manager; Office viewing; Audio decision; Store backend against real OS | Per-app tests | Store security items |
| 6 Packages and updates | Package format, signed catalog with freshness, privileged installer, update and rollback | Design reviewed first; then tests including rollback | boot trust bridge; kernel isolation |
| 7 Install and release | ISO, installer, first boot, recovery, VM and hardware tests, release signing, reproducible builds | A VM boots an installed system from release media; reproducibility checked by two builds | phases 2, 6 |

Rules for every phase: small reviewable change sets, tests with the code, failures reported not hidden, no production claims for experimental parts.
