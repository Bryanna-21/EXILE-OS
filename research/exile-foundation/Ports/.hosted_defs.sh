#!/usr/bin/env bash

SCRIPT="$(realpath $(dirname "${BASH_SOURCE[0]}"))"

export EXILE_SOURCE_DIR="$(realpath "${SCRIPT}/../")"

. "${EXILE_SOURCE_DIR}/Meta/shell_include.sh"

export EXILE_ARCH="${EXILE_ARCH:-${HOST_ARCH}}"
export EXILE_TOOLCHAIN="${EXILE_TOOLCHAIN:-GNU}"

if [ -z "${HOST_CC:=}" ]; then
    export HOST_CC="${CC:=cc}"
    export HOST_CXX="${CXX:=c++}"
    export HOST_LD="${LD:=ld}"
    export HOST_AR="${AR:=ar}"
    export HOST_RANLIB="${RANLIB:=ranlib}"
    export HOST_PATH="${PATH:=}"
    export HOST_READELF="${READELF:=readelf}"
    export HOST_OBJCOPY="${OBJCOPY:=objcopy}"
    export HOST_OBJDUMP="${OBJDUMP:=objdump}"
    export HOST_STRIP="${STRIP:=strip}"
    export HOST_CXXFILT="${CXXFILT:=c++filt}"
    export HOST_PKG_CONFIG_DIR="${PKG_CONFIG_DIR:=}"
    export HOST_PKG_CONFIG_SYSROOT_DIR="${PKG_CONFIG_SYSROOT_DIR:=}"
    export HOST_PKG_CONFIG_LIBDIR="${PKG_CONFIG_LIBDIR:=}"
    export HOST_PKG_CONFIG_PATH="${PKG_CONFIG_PATH:=}"
fi

if [ "$EXILE_TOOLCHAIN" = "Clang" ]; then
    export EXILE_BUILD_DIR="${EXILE_SOURCE_DIR}/Build/${EXILE_ARCH}clang"
    export EXILE_TOOLCHAIN_BINDIR="${EXILE_SOURCE_DIR}/Toolchain/Local/clang/bin"
    export CC="${EXILE_ARCH}-serenity-clang"
    export CXX="${EXILE_ARCH}-serenity-clang++"
    export LD="${EXILE_TOOLCHAIN_BINDIR}/ld.lld"
    export AR="llvm-ar"
    export RANLIB="llvm-ranlib"
    export READELF="llvm-readelf"
    export OBJCOPY="llvm-objcopy"
    export OBJDUMP="llvm-objdump"
    export STRIP="llvm-strip"
    export CXXFILT="llvm-cxxfilt"
else
    export EXILE_BUILD_DIR="${EXILE_SOURCE_DIR}/Build/${EXILE_ARCH}"
    export EXILE_TOOLCHAIN_BINDIR="${EXILE_SOURCE_DIR}/Toolchain/Local/${EXILE_ARCH}/bin"
    export CC="${EXILE_ARCH}-serenity-gcc"
    export CXX="${EXILE_ARCH}-serenity-g++"
    export LD="${EXILE_TOOLCHAIN_BINDIR}/${EXILE_ARCH}-serenity-ld"
    export AR="${EXILE_ARCH}-serenity-ar"
    export RANLIB="${EXILE_ARCH}-serenity-ranlib"
    export READELF="${EXILE_ARCH}-serenity-readelf"
    export OBJCOPY="${EXILE_ARCH}-serenity-objcopy"
    export OBJDUMP="${EXILE_ARCH}-serenity-objdump"
    export STRIP="${EXILE_ARCH}-serenity-strip"
    export CXXFILT="${EXILE_ARCH}-serenity-c++filt"
fi

export PATH="${EXILE_TOOLCHAIN_BINDIR}:${EXILE_SOURCE_DIR}/Toolchain/Local/cmake/bin:${HOST_PATH}"

export PKG_CONFIG_DIR=""
export PKG_CONFIG_SYSROOT_DIR="${EXILE_BUILD_DIR}/Root"
export PKG_CONFIG_LIBDIR="${PKG_CONFIG_SYSROOT_DIR}/usr/local/lib/pkgconfig"
export PKG_CONFIG_PATH="${PKG_CONFIG_LIBDIR}"

export SERENITY_INSTALL_ROOT="${EXILE_BUILD_DIR}/Root"
export SERENITY_PORT_DIRS="${SERENITY_PORT_DIRS:+${SERENITY_PORT_DIRS}:}${EXILE_SOURCE_DIR}/Ports"
