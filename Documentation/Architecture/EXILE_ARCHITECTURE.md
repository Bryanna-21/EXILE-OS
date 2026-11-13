# EXILE-OS Version 1 — Architectural Boundary & Polyglot Naming Manifest

**Baseline Commit:** `e827badd91264c4d40bd7af5a9fe9ba0412cab6d`  
**OS Generation:** Version 1 (`EXILE_OS_VERSION=1`)  
**Supported Runtimes:** C++ (Core/Legacy), Rust (EUK), Python/TypeScript (Tooling & Services)

---

## 1. Core Architectural Partitioning

Version 1 operates as a **dual-kernel, polyglot hybrid system**. To prevent cross-contamination between inherited C++ upstream subsystems, custom Rust kernel components, and multi-language tooling—and to ensure clean separation when Version 2 is introduced—all directories adhere to strict ownership boundaries.

| Subsystem Domain | Directory Path | Language Stack | Ownership | Modification Policy |
| :--- | :--- | :--- | :--- | :--- |
| **Legacy Monolithic Kernel** | `Kernel/` | C++ | Inherited (SerenityOS) | **Protected.** Preserve upstream layout and CMake targets. |
| **Exile Universal Kernel (EUK)** | `ExileKernel/` | Rust | Exile-Authored | **Isolated.** Strictly segregated from `Kernel/`. |
| **Foundational Runtime** | `AK/` | C++ | Inherited (SerenityOS) | **Protected.** Never rename; breaks global include paths. |
| **Browser Engine** | `Ladybird/` | C++ | Third-Party / Upstream | **Protected.** Preserve upstream sync compatibility. |
| **Third-Party Packages** | `Ports/` | Mixed / Recipes | Third-Party | **Protected.** Do not alter upstream package names. |
| **User-Space Ecosystem** | `Userland/` | C++ / Polyglot | Mixed | **Scoped.** Native Exile apps must be clearly identifiable (`ExileStore`, `Bridgeline`, `XFiles`). |
| **Build & Patch Metadata** | `Meta/` & `Meta/Patches/` | Python / CMake / Diff | Mixed | **Normalized.** All project-wide patches and scripts live here. |

---

## 2. Polyglot & Multi-Version Naming Contract (v1 -> v2 Compatibility)

To prevent path collisions, IDE indexing confusion, and build-script ambiguity across future OS generations (`v1` vs `v2`), all new Exile-authored modules must follow these rules:

1. **No Unscoped Generic Root Folders:** Never create root directories named `utils/`, `scripts/`, `core/`, or `services/`. Every custom folder must either carry the `Exile` prefix or reside inside a scoped parent (`Meta/`, `Userland/`, `ExileKernel/`).
2. **Language-Appropriate Casing:**
   - **C++ / Rust Subsystem & App Directories:** `PascalCase` (e.g., `ExileKernel/`, `ExileStore/`, `Bridgeline/`).
   - **Python / TypeScript / Shell Tooling & Scripts:** `kebab-case` or `snake_case` (e.g., `verify_kernel.py`, `build-exile.sh`).
3. **Version Isolation:** Any experimental component intended solely for the next-generation architecture must be quarantined under `Meta/Experiments/` or a dedicated `v2` workspace rather than mixed into Version 1's active build graph.
