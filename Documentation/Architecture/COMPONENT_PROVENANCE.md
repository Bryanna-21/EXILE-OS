# Component provenance

Distinguishes where each part of Exile OS comes from. Evidence is based on source headers and repository documents. File counts are a historical snapshot taken from `origin/main` at `d0b61526`, excluding `Ports/` and `Ladybird/`. They are not a fresh count of the integration branch, which is based on `origin/main` at `c8edf3d0`. Whether a file is *modified* from upstream is judged by its header and the historical audit, not by a complete diff against upstream.

| Class | What | Evidence |
|---|---|---|
| **Serenity-derived** | The great majority of the OS: `Kernel/` (C++ kernel), `AK/`, `Userland/Libraries/`, most of `Userland/Services` and `Userland/Applications`, build system | `Meta/Exile/IDENTITY.md` ("originated from the SerenityOS codebase"); 1,460 files carry the notice "SerenityOS developers"; root `LICENSE` is BSD-2-Clause with the upstream copyright |
| **Exile-modified (Serenity-derived)** | XFiles (a fork of FileManager with `TrashActions`), Bridgeline (a fork of Browser), the Exile branding | `Userland/Applications/XFiles`, `Userland/Applications/Bridgeline`; the audit's diff counts (158 and 87 lines). Which other files were modified for branding is not established |
| **Exile-original** | Boot integrity (`Kernel/Boot/Exile/*`, `Kernel/EFIPrekernel/BootVerification.*`, `TrustAnchors.cpp.in`, `Meta/Tools/ExileBootManifest.cpp`, `Tests/ExileBoot/*`); Exile Store (62 files under `Userland/Applications/ExileStore`); `LibFileSystem` Trash; `BootSplash` | Files carrying "the Exile OS developers" (82 files under `Kernel/`, `Userland/`, `Tests/`, `Meta/` and `Documentation/`) |
| **Experimental** | Verified boot (opt-in, off by default); Exile Store (self-described "not production-secure"); `Meta/Experiments/OMNI-Kernel-Batch-*` (hosted prototypes, not integrated); `ExileKernel/` (EUK, see below) | `Documentation/BootIntegrity.md`; `ExileKernel/README.md`; `Meta/Experiments/OMNI-Kernel-Batch-5/EXILE-OS-STATUS.md` |
| **Planned replacements** | An independent kernel (EUK); hybrid architecture | `Documentation/Project-Audits/EUK_INTEGRATION_PLAN.md` |
| **Unimplemented** | Installer, ISO, recovery, update service, firewall, Wi-Fi, log and time services, graphical archive manager, application sandbox enforcement | `NON_KERNEL_SYSTEM_AUDIT_ADDENDUM.md` |

## What is not claimed

* **Exile does not have an independent kernel.** The running OS uses the inherited `Kernel/`. `ExileKernel/` is a separate Rust milestone that, by its own README, "is not an Exile userspace-capable kernel" and is built and tested by its own scripts and workflow (`.github/workflows/euk.yml`).
* **Hardware support is the inherited set**: e1000 and Realtek 8168 network, AC'97 and Intel HDA audio, xHCI and UHCI USB (`Documentation/BareMetalInstallation.md`). No Wi-Fi, no GPU acceleration.
* **Licence and attribution are preserved**; nothing in this document changes them. Any replacement of a Serenity-derived component needs the compatibility and licensing audit the project rules require first.
