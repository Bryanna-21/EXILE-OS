# Exile OS

Exile OS is an independent operating-system project focused on user control, privacy, and a coherent desktop experience. It is developed under the Exile Organization and builds on a substantial inherited codebase while adding original Exile-authored components.

> **Development status:** This project is under active early-stage development. The inherited codebase provides a working graphical Unix-like system for x86-64. Exile-authored components — including the Exile Universal Kernel (EUK) and Exile Store — are experimental and not yet integrated into the primary boot image. Nothing in this document should be read as a claim that any architecture or feature beyond the inherited codebase has been independently validated for production use.

---

## Vision and principles

Exile OS is being built around a small set of commitments:

Exile OS is being built around a small set of commitments:

- **User sovereignty.** The project aims to give users control over what runs on their machines, what data is stored, and what is transmitted. Many core operating-system tasks can run without network access, while network-dependent features require connectivity.
- **Privacy by design.** Privacy is a design goal, not a claim of complete system-wide verification. The project aims to minimize unnecessary telemetry and prevent page contents from being sent to external AI services without explicit user authorization. Privacy behavior depends on the implementation and configuration of inherited components and connected services. Site data isolation and clearing depend on each component's storage implementation.
- **Transparent system behavior.** The inherited codebase includes security mechanisms such as pledge, unveil, W^X memory protections, ASLR, and process isolation. Their presence does not establish that every component is correctly configured or independently audited. Certificate validation and other security behavior depend on the relevant implementation and configuration.
- **Native applications and coherent UX.** Applications share a common toolkit (LibGUI), icon language, and design philosophy. The interface is consistent across the desktop, and the long-term ambition includes a shared desktop and mobile ecosystem.
- **Extensibility.** A port system provides access to hundreds of open-source packages. The architecture supports future engine adapters and compatibility layers, subject to licensing, ABI compatibility, and security review.

---

## Architecture

Exile OS combines an inherited userland with new Exile-authored kernel work.

### Inherited components

The majority of the repository is derived from the SerenityOS codebase and is used under its BSD-2-Clause license. These components provide:

- A modern 64-bit kernel (`Kernel/`) with pre-emptive multi-threading, hardware protection features, and a POSIX-compatible system-call interface.
- A full graphical desktop environment built on LibGUI, with WindowServer, LoginServer, AudioServer, and supporting services.
- LibWeb and the inherited browser engine, providing HTML5 parsing, CSS, JavaScript, and WebAssembly support.
- A C standard library (LibC), POSIX-compatible shell, virtual file systems (/proc, /dev, /sys), and an ext2 filesystem.
- A network stack supporting IPv4, TCP, UDP, DNS, HTTP, TLS, and other protocols.
- A large collection of native applications, libraries, games, and utilities.
- A port system with over three hundred third-party software packages.

These components have not been independently re-validated for Exile OS beyond what the inherited CI infrastructure verifies.

### Exile-authored components

Exile OS adds original components alongside the inherited codebase:

- **Exile Universal Kernel (EUK).** An experimental Rust-based kernel in `ExileKernel/`, currently targeting isolated x86-64 boot as milestone 1. EUK does not replace the inherited kernel and does not yet support userspace. It is developed separately with its own toolchain, test harness, and verification documentation. See `ExileKernel/README.md` and `ExileKernel/docs/` for details.
- **Exile Store.** A native catalog browser for discovering and installing software, added to `Userland/Applications/`.
- **Boot integrity tooling.** Verification infrastructure documented in `Documentation/BootIntegrity.md`.
- **Project audits.** Structured reviews of application inventory, branding, licensing, and integration gaps, maintained in `Documentation/Project-Audits/`.
- **Exile file format documentation.** MIME detection policy and format specifications in `Documentation/ExileFileFormats.md`.

### Platform support

| Architecture | Status |
|---|---|
| x86-64 | Inherited kernel builds and boots via CI. EUK milestone 1 provides experimental isolated boot. |
| AArch64 (ARM64) | Inherited codebase includes ARM64 port. Build and boot status tracked in `Documentation/Project-Audits/`. |
| RISC-V | Inherited codebase includes RISC-V port documentation (`Documentation/RISC-V.md`). Not validated by Exile OS CI. |

Architecture validation status is tracked in `Documentation/Project-Audits/`. Do not assume an architecture works because it appears in the source tree.

---

## Repository structure

```
AK/                    Core C++ template and utility library
Base/                  Base filesystem image: man pages, config, resources, etc.
Documentation/         Build instructions, coding style, architecture docs, project audits
ExileKernel/           Exile Universal Kernel (EUK) — experimental Rust kernel
Kernel/                Inherited system kernel (C++)
Ladybird/              Inherited browser engine and standalone browser build
Meta/                  Project metadata, Exile identity, CI scripts, linting
  Meta/Exile/          Exile-specific identity and branding
Ports/                 Third-party software port definitions
Tests/                 Test infrastructure for libraries and kernel
Toolchain/             Cross-compilation toolchain build scripts
Userland/              Userspace code
  Userland/Applications/  GUI applications (Browser, Store, Terminal, etc.)
  Userland/Libraries/     System libraries (LibGUI, LibWeb, LibC, LibCore, etc.)
  Userland/Services/      System services (WindowServer, AudioServer, etc.)
  Userland/Utilities/     Command-line utilities
  Userland/Games/         Games
  Userland/Demos/         Demo applications
patches/               Patches applied to third-party code
```

---

## Getting started

### Prerequisites

Building Exile OS requires a Unix-like host environment. The build system supports cross-compilation from Linux, macOS, and Windows (via WSL2). Detailed, tested instructions are in `Documentation/BuildInstructions.md`.

Host-specific guides:

- [Building on Linux](Documentation/BuildInstructionsOther.md)
- [Building on macOS](Documentation/BuildInstructionsMacOS.md)
- [Building on Windows (WSL2)](Documentation/BuildInstructionsWindows.md)

A Nix flake (`flake.nix`) is provided for reproducible development environments.

### Building the inherited system

```sh
# Clone the repository
git clone https://github.com/Bryanna-21/EXILE-OS.git
cd EXILE-OS

# Build the toolchain (first time only)
Toolchain/BuildIt.sh

# Configure and build
cmake -S Meta/CMake/Superbuild -B Build/superbuild -GNinja
cmake --build Build/superbuild

# Run in QEMU
./Meta/serenity.sh run
```

These commands are inherited from the upstream build system. If they fail on your host, consult `Documentation/Troubleshooting.md` and `Documentation/BuildInstructions.md` before opening an issue.

### Building Ladybird (standalone browser)

Ladybird can be built separately from the full OS. See `Documentation/BuildInstructionsLadybird.md`.

### Building the Exile Universal Kernel (EUK)

EUK is built independently from the main OS. It requires Rust 1.91.1 (checksum-verified), GNU gcc/binutils, and Python 3. No crates or OS headers are needed.

```sh
cd ExileKernel
export EUK_RUSTC=/path/to/rust-1.91.1/bin/rustc
export EUK_SYSROOT=/path/to/rust-1.91.1
python3 scripts/test.py
python3 scripts/build.py
python3 scripts/boot_test.py
```

See `ExileKernel/README.md` and `ExileKernel/docs/TOOLCHAIN.md` for exact toolchain requirements and checksums.

---

## Testing

Exile OS inherits a test infrastructure from the upstream codebase. Tests are located in `Tests/` and cover libraries, kernel subsystems, and utilities.

```sh
# Run tests (inherited)
./Meta/serenity.sh test
```

EUK provides its own test harness:

```sh
cd ExileKernel
python3 scripts/test.py
```

See `Documentation/RunningTests.md` for details on the inherited test infrastructure.

**What has been verified:** The active CI workflow runs selected inherited build and test jobs on pushes and pull requests across configured toolchain, architecture, and debug configurations. This does not establish that every test suite passes on every change. EUK milestone 1 has a separate experimental test and boot-validation workflow. No broader integration testing between EUK and the inherited userland has been performed.

---

## Development and contribution

Exile OS welcomes contributions. Before starting, read:

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — Code submission policy, commit message format, coding style
- [`Documentation/CodingStyle.md`](Documentation/CodingStyle.md) — Style guide
- [`Documentation/Patterns.md`](Documentation/Patterns.md) — Common development patterns

### Guidelines

- **Start small.** For your first contributions, fix a bug or improve documentation before adding new applications or large components.
- **One change per commit.** Keep commits atomic and well-described. Use the format `Category: Brief description`.
- **Write tests.** Include tests when fixing bugs or adding features.
- **Use American English.** The official project language is American English with ISO 8601 dates and metric units.
- **Check the audits.** Before adding applications or changing branding, review `Documentation/Project-Audits/` for existing guidance.

### Issue policy

File bugs on GitHub. One issue per bug. Do not file build issues if CI passes — work those out locally or ask for help. Include complete debug logs and hardware details for bare-metal issues.

---

## Project status and roadmap

### Implemented

- Inherited graphical desktop environment (x86-64)
- Inherited kernel with security features (pledge, unveil, ASLR, W^X)
- Inherited browser with LibWeb engine
- Inherited network stack and TLS
- Inherited POSIX compatibility layer
- Exile project identity and branding (`Meta/Exile/IDENTITY.md`)
- Exile Store catalog browser
- Boot integrity documentation
- Project audit infrastructure
- EUK milestone 1 (isolated x86-64 boot, experimental)

### Experimental

- Exile Universal Kernel (EUK) — Rust-based, no userspace yet
- AArch64 and RISC-V architecture ports (inherited, not Exile-validated)

### Planned

- EUK integration with Exile OS userspace (subject to milestone completion)
- Engine adapter architecture for cross-browser compatibility (see Bridgeline browser project)
- Local-first AI integration boundary
- Mobile ecosystem convergence (long-term)
- Expanded architecture validation and CI

### Not supported

- Production deployment on any architecture
- Security audit of inherited components beyond upstream CI
- Compatibility guarantees with third-party software not in the port system

---

## Licensing and provenance

Exile OS contains code from multiple sources with different licenses.

### Inherited codebase

The majority of the repository is derived from SerenityOS and is licensed under the **BSD 2-Clause License**. Copyright is held by the SerenityOS developers. All upstream copyright notices and license terms are preserved. See [`LICENSE`](LICENSE) for the full text.

### Exile-authored components

Exile-authored components are added under compatible terms. The Exile Universal Kernel (EUK) milestone 1 source is released under the **Apache License 2.0**. See `ExileKernel/LICENSE`, `ExileKernel/NOTICE.md`, and `ExileKernel/THIRD_PARTY.md` for details.

### Third-party components

The port system (`Ports/`) includes third-party software under their respective licenses. Individual port directories contain license information. The `patches/` directory contains modifications to third-party code.

### Provenance notice

This project originated from the SerenityOS codebase. Exile OS is an independent project with its own identity, goals, and roadmap, but the majority of the code was not originally authored for Exile OS. Changing the README does not change the provenance of inherited code. All legally required copyright notices, license terms, and attribution are preserved.

See `Documentation/Project-Audits/LICENSING_AND_PROVENANCE_AUDIT.md` for a detailed provenance review.

---

## Documentation

- [Build Instructions](Documentation/BuildInstructions.md)
- [Ladybird Build Instructions](Documentation/BuildInstructionsLadybird.md)
- [Coding Style](Documentation/CodingStyle.md)
- [Running Tests](Documentation/RunningTests.md)
- [Boot Integrity](Documentation/BootIntegrity.md)
- [Exile File Formats](Documentation/ExileFileFormats.md)
- [FAQ](Documentation/FAQ.md)
- [Project Audits](Documentation/Project-Audits/)

Man pages are generated from `Base/usr/share/man` and available online. Within the OS, use `man` for the terminal interface or `help` for the GUI.

---

## Security

Exile OS inherits security mechanisms from its underlying codebase, including hardware protections, userland capability restrictions, W^X memory, pledge and unveil, ASLR, and web-content process isolation. The system also includes LibTLS for TLS functionality. The exact protections available depend on the implementation, build configuration, and runtime configuration.
However, this is an early-stage project. The inherited codebase has not received an independent security audit. Exile-authored components are experimental. Do not deploy Exile OS in security-critical environments without your own review.

See [`SECURITY.md`](SECURITY.md) for the vulnerability reporting policy.

---

## Project identity

Exile OS is developed under the **Exile Organization**, founded by the **Exile Foundation**. It is an independent operating-system project with its own identity, goals, and roadmap.

Public-facing branding uses **Exile OS**. Technical identifiers are migrated deliberately when compatibility and build requirements allow. The project identity is documented in `Meta/Exile/IDENTITY.md`.

---

## Acknowledgments

Exile OS builds on the work of the SerenityOS developers and community. The inherited codebase represents years of work by hundreds of contributors. Exile OS preserves all upstream attribution and licensing.

The Exile-authored components are developed by the Exile Organization and contributors. See the [contributor graph](https://github.com/Bryanna-21/EXILE-OS/graphs/contributors) for the full list.

---

## Contact

- **Repository:** https://github.com/Bryanna-21/EXILE-OS
- **Issues:** https://github.com/Bryanna-21/EXILE-OS/issues
- **Security:** See [`SECURITY.md`](SECURITY.md)

---

*Exile OS — User control. Privacy. Coherent experience.*
