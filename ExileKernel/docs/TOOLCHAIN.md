# Pinned target toolchain

Rust compiler: https://static.rust-lang.org/dist/rustc-1.91.1-x86_64-unknown-linux-gnu.tar.xz
SHA256: 4b4c596fc5268435c310a79c2e231a5a3567572c930ac0740ef9e147e83baf4e

Bare-metal std target: https://static.rust-lang.org/dist/rust-std-1.91.1-x86_64-unknown-none.tar.xz
SHA256: 069edc1d99b45efd97f1d9746289667dc4dc38d8532da547bcc3d0d735420d16

Download to a temporary location, verify against these digests before extracting, then run each archive's install.sh --prefix=/your/toolchain --disable-ldconfig. Set EUK_RUSTC and EUK_SYSROOT to that compiler and prefix. Keep the host compiler's std for portable tests via EUK_HOST_RUSTC. Do not mix distribution-rebuilt core libraries with the official compiler: matching version labels alone do not prove crate metadata compatibility.

Observed GNU GCC 14.3.0, binutils 2.44, rustfmt 1.91.1, GRUB 2.12, QEMU 10.1.2, xorriso 1.5.6. See VERIFICATION.md for exact environment paths. Byte-identical ELF is verified on this environment; cross-host/tool-version reproducibility and reproducible ISO generation are NOT claimed. CI pins Rust digests and OS release, not every apt package; hard-pin the entire environment before broader reproducibility claims.
