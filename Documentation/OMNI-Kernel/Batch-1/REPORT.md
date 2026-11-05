# OMNI-Kernel Batch 1 — Delivery and Validation Report

**Scope:** Batch 1 is a specification-only milestone. No kernel code, bootloader, build system, or tests were produced. This report states what was produced, what was checked, and what was not.

## 1. Files produced

| File | Purpose and scope | Approx. size |
|---|---|---|
| `README.md` | Project identity, Exile OS relationship, Zig-first principles, targets, execution modes, WebAssembly role, security principles, staged roadmap, status table | about 1,570 words |
| `docs/ARCHITECTURE.md` | Seven-layer model, trust boundaries, kernel versus user-space trade-offs, boot phases, memory, scheduling, IPC, capabilities, drivers, services, observability, backends, portability, testing, Exile OS integration, open questions, targets and non-goals | about 3,880 words |
| `docs/ABI-AND-SECURITY.md` | Common ABI conventions, security model and threat model, and proposed contracts for `omni_fs`, `omni_net`, `omni_process`, `omni_ipc`, `omni_clock`, `omni_random`, `omni_gui`, plus open design decisions | about 5,980 words |
| `REPORT.md` | This report | n/a |
| `manifest.json` | SHA-256 checksums and byte sizes for the four files above; excludes itself | n/a |

The task text says the ZIP must contain "all six files" but lists five (four documents plus the manifest, with the directory structure shown). The package contains those five files. No sixth file was invented.

## 2. Architectural decisions made

- Seven downward-only layers; hosted, virtualized, and bare-metal treated as distinct execution environments.
- Capability model with kernel-held entries and opaque per-process handles; default-deny; no ambient authority.
- Minimal privileged kernel: address spaces, threads and scheduler core, IPC, capability table, interrupt routing, monotonic clock and entropy pool. Drivers, filesystems, networking, and GUI are user-space services.
- Memory objects as the unit of shared memory; paths in `omni_fs` are always relative to a held directory handle.
- Single ABI label **OMNI-ABI 0.1 (draft)**; each of the seven interfaces is at version 0.1 with no compatibility promise while the major version is 0.
- Provisional choices recorded with rationale: revocation (derivation tree plus service-side indirection), cancellation (operation objects with explicit states), shared-object accounting (charge creator), path normalization (none), `CRYPTO` randomness granted by default.

## 3. Known limitations and unresolved questions

- Nothing is implemented. All interfaces are proposed; pseudocode is conceptual and is not valid Zig. No ABI header exists, and numeric error codes and binary layouts are not assigned.
- Open questions are tabulated in `docs/ARCHITECTURE.md` (section 15.3, Q1–Q7) and `docs/ABI-AND-SECURITY.md` (Part E, D1–D7), including capability revocation, IPC priority inversion, Wasm host interface, debugging capability, Zig version pinning, DMA protection without an IOMMU, and mapping of Exile OS components.
- Batch 1 did not inspect any Exile OS source. Integration statements are intent only.
- Side channels, physical attacks, firmware compromise, and supply-chain attacks are out of scope of the threat model.
- No security, performance, or portability claim has been tested or reviewed externally.

## 4. Checks actually run

All commands were run in this session on the working copy. Helper scripts were kept outside the package.

**Content checks on `README.md`, `docs/ARCHITECTURE.md`, `docs/ABI-AND-SECURITY.md` (scripted):**

- Word and byte counts: computed. ARCHITECTURE is within the requested range of roughly 2,500–4,000 words.
- Placeholder scan for the usual unfinished-work markers and filler text: no matches in these three files.
- Backticked file references: all references resolved to existing files, except `REPORT.md` and `manifest.json`, which did not exist at the time and are checked in the final pass.
- Inline Markdown links: none present in the documents, so there were no link targets to check.
- ABI version label consistency: one inconsistent spelling in `README.md` was found and corrected. All documents now use "OMNI-ABI 0.1 (draft)". The seven interface versions in the ABI document were all found at 0.1.
- All seven interface names occur in all three documents.
- Internal "section N" cross-references in the documents were extracted and compared against the actual section numbers by manual inspection of the printed list; all match existing sections.
- Scan for claim-like wording ("implemented", "formally verified", "production-ready", "passes", "already boots"): every hit is a negation or a disclaimer.

**Packaging checks (pass 1, on the draft package before this report was written):**

- Directory contained exactly the five expected files.
- `manifest.json` parsed with Python's JSON parser; four entries; `manifest.json` absent from its own list.
- Every SHA-256 and byte size recomputed from the real files and matched; independently cross-checked with the `sha256sum` tool.
- ZIP created with `zip`; listing showed each file once and no forbidden content (`.git`, `node_modules`, `dist`, caches, temporary files); `unzip -t` reported no errors.
- Fresh extraction of that ZIP into a new directory and re-verification: all entries matched.
- Scan of the extracted package for machine-specific absolute paths: none found.

Pass 1 used a one-line temporary `REPORT.md`, which this file replaced. Because this report changed the package contents, the manifest and ZIP were regenerated afterwards.

## 5. Checks not run

- No Markdown renderer was run, and the Mermaid diagram in `docs/ARCHITECTURE.md` was not rendered or syntax-checked.
- No spell-check or grammar tool was run.
- No independent human or external technical review took place; consistency review beyond the scripted checks was performed by the author only.
- No Zig, compiler, or code of any kind was run, because none exists. Pseudocode was not compiled.
- Extraction was tested on Linux only (not Windows or macOS tools).
- No test of kernel behavior, security controls, randomness quality, or revocation and cancellation semantics was possible or attempted.

## 6. Remaining risks and contradictions

- Terminology is consistent per the scripted checks, but only partly by author reading; subtle wording conflicts between documents may remain.
- `docs/ABI-AND-SECURITY.md` section A.1 leaves the trailing-byte rule (zero or reject) to be chosen per minor version, which is deliberately unresolved.
- Some provisional recommendations (for example revocation R2 plus R1) are untested design judgments and could change after prototyping.
- Cancellation, revocation, accounting, and DMA protection are described as desired semantics only; implementation feasibility is unproven.

## 7. Final verification

The results of the final rebuild and final fresh-extraction verification, the ZIP size, and the ZIP's SHA-256 are not recorded in this file, because recording them would change this file and invalidate the manifest and archive they describe. They are reported in the delivery message that accompanies the ZIP. The ZIP's own SHA-256 cannot be placed inside the ZIP for the same reason.
