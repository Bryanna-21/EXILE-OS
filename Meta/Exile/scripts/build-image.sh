#!/usr/bin/env bash
# Build a disk image with the repository's existing image infrastructure and validate it.
#
# Uses the CMake image targets that Meta/exile.sh uses (qemu-image, limine-image, grub-image, extlinux-image), which run the
# scripts Meta/build-image-*.sh. Those scripts need root (loop devices, mount) or fuse2fs/genext2fs, and the Limine and GRUB
# variants need extra host tools; this wrapper checks for them first and reports BLOCKED instead of failing half way.
#
# The default 'qemu' image is a raw ext2 root filesystem. QEMU loads the kernel directly (-kernel), so that image has no
# bootloader in it. Limine, GRUB and extlinux images are partitioned disks with a bootloader.
#
# Exit codes: 0 success, 1 usage, 2 missing host tool, 3 BLOCKED (kernel or installed userland missing), 4 build step failed,
#             5 the image was produced but failed validation.
# Last line: RESULT <PASS|FAIL|BLOCKED|USAGE> step=<name> reason=<text>
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ARCH="${EXILE_ARCH:-x86_64}"
TOOLCHAIN_SUFFIX=""
BOOTLOADER="qemu"
OUT="${EXILE_VALIDATION_OUT:-$ROOT/Build/m1-validation}"
VALIDATE_ONLY=""
JOBS="${MAKEJOBS:-2}"
MIN_IMAGE_BYTES=$((16 * 1024 * 1024))

usage() {
    cat <<EOF
Usage: ${0##*/} [--bootloader qemu|limine|grub|extlinux] [--arch x86_64] [--out DIR] [--validate-only IMAGE]

  --bootloader   Which existing image target to build (default: qemu = raw ext2 image used with QEMU -kernel).
  --validate-only IMAGE
                 Do not build; run the image validation on an existing file (works without root or a toolchain).
Requires a finished 'Meta/exile.sh build $ARCH GNU' (kernel plus installed userland) before a real image can be built.
EOF
}
result() { echo "RESULT $1 step=$2 reason=$3"; exit "$4"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --bootloader) BOOTLOADER="${2:-}"; shift 2 || result USAGE args "--bootloader needs a value" 1 ;;
        --arch) ARCH="${2:-}"; shift 2 || result USAGE args "--arch needs a value" 1 ;;
        --out) OUT="${2:-}"; shift 2 || result USAGE args "--out needs a value" 1 ;;
        --validate-only) VALIDATE_ONLY="${2:-}"; shift 2 || result USAGE args "--validate-only needs a file" 1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; result USAGE args "unknown argument: $1" 1 ;;
    esac
done
case "$BOOTLOADER" in
    qemu) TARGET=qemu-image; IMAGE_NAME=_disk_image ;;
    limine) TARGET=limine-image; IMAGE_NAME=limine_disk_image ;;
    grub) TARGET=grub-image; IMAGE_NAME=grub_disk_image ;;
    extlinux) TARGET=extlinux-image; IMAGE_NAME=extlinux_disk_image ;;
    *) result USAGE args "unknown --bootloader '$BOOTLOADER'" 1 ;;
esac
case "$ARCH" in x86_64) ;; *) result USAGE args "only x86_64 images are supported by this script" 1 ;; esac
BUILD_DIR="$ROOT/Build/$ARCH$TOOLCHAIN_SUFFIX"

# ------------------------------------------------------------------------------ validation (pure read, no root needed)
# Checks: size, partition table (MBR / GPT) consistency, an ext2 superblock magic on the root filesystem, and, when debugfs is
# available and a kernel binary is given, that /boot/Kernel inside the image is byte-identical to that kernel.
validate_image() { # image [kernel-for-comparison]
    local image="$1" kernel="${2:-}"
    [ -f "$image" ] || { echo "validate: $image does not exist" >&2; return 1; }
    local size; size="$(stat -c %s "$image")"
    if [ "$size" -lt "$MIN_IMAGE_BYTES" ]; then echo "validate: $image is only $size bytes (< $MIN_IMAGE_BYTES)" >&2; return 1; fi
    local report offset=""
    report="$(python3 -I - "$image" "$BOOTLOADER" <<'PY'
import struct, sys
path, bootloader = sys.argv[1], sys.argv[2]
EXT2_MAGIC = b"\x53\xef"
def ext2_at(f, offset):
    f.seek(offset + 1024 + 56)
    return f.read(2) == EXT2_MAGIC
with open(path, "rb") as f:
    f.seek(0, 2); size = f.tell()
    f.seek(510); sig = f.read(2)
    parts = []   # (start_byte, length_bytes, label)
    if sig == b"\x55\xaa":
        f.seek(446); table = f.read(64)
        for i in range(4):
            e = table[i * 16:(i + 1) * 16]
            ptype, lba, sectors = e[4], struct.unpack("<I", e[8:12])[0], struct.unpack("<I", e[12:16])[0]
            if ptype and ptype != 0xEE and sectors:
                parts.append((lba * 512, sectors * 512, f"mbr{i}:0x{ptype:02x}"))
        gpt = any(table[i * 16 + 4] == 0xEE for i in range(4))
    else:
        gpt = False
    if gpt:
        f.seek(512); hdr = f.read(92)
        if hdr[:8] != b"EFI PART":
            print("validate: protective MBR present but no GPT header", file=sys.stderr); sys.exit(1)
        entries_lba, count, esize = struct.unpack("<QII", hdr[72:88])
        f.seek(entries_lba * 512)
        for i in range(count):
            e = f.read(esize)
            if len(e) < 128 or e[:16] == b"\0" * 16: continue
            first, last = struct.unpack("<QQ", e[32:48])
            parts.append((first * 512, (last - first + 1) * 512, "gpt%d" % i))
    if bootloader == "qemu":
        # Raw filesystem, no partition table: the superblock sits at the very start.
        if not ext2_at(f, 0):
            print("validate: no ext2 superblock at offset 0 (expected a raw ext2 image)", file=sys.stderr); sys.exit(1)
        print("validate: raw ext2 filesystem, %d bytes" % size)
    else:
        if sig != b"\x55\xaa":
            print("validate: missing MBR boot signature 0x55AA", file=sys.stderr); sys.exit(1)
        if not parts:
            print("validate: no partitions found", file=sys.stderr); sys.exit(1)
        for start, length, label in parts:
            if start + length > size:
                print("validate: partition %s extends past the end of the file" % label, file=sys.stderr); sys.exit(1)
        ext2 = [p for p in parts if ext2_at(f, p[0])]
        if not ext2:
            print("validate: none of %d partitions holds an ext2 filesystem" % len(parts), file=sys.stderr); sys.exit(1)
        print("validate: %d partitions, ext2 root at byte offset %d" % (len(parts), ext2[0][0]))
        print("EXT2_OFFSET=%d" % ext2[0][0])
PY
)" || return 1
    echo "$report" | grep -v '^EXT2_OFFSET='
    offset="$(echo "$report" | sed -n 's/^EXT2_OFFSET=//p')"
    echo "validate: structure OK"
    if [ -n "$kernel" ] && [ -f "$kernel" ] && command -v debugfs >/dev/null 2>&1 && [ "$BOOTLOADER" != limine ]; then
        local src="$image"
        [ -n "$offset" ] && src="$image?offset=$offset"
        local tmp; tmp="$(mktemp)"; rm -f "$tmp"
        if debugfs -R "dump /boot/Kernel $tmp" "$src" >/dev/null 2>&1 && [ -s "$tmp" ]; then
            if cmp -s "$tmp" "$kernel"; then echo "validate: /boot/Kernel in the image is identical to $kernel"
            else rm -f "$tmp"; echo "validate: /boot/Kernel in the image differs from $kernel" >&2; return 1; fi
        else
            echo "validate: /boot/Kernel not found inside the image (not verified)"
        fi
        rm -f "$tmp"
    else
        echo "validate: kernel presence inside the image NOT verified (needs debugfs, a kernel binary, and a non-Limine layout)"
    fi
    return 0
}

if [ -n "$VALIDATE_ONLY" ]; then
    validate_image "$VALIDATE_ONLY" "${EXILE_KERNEL_FOR_COMPARE:-}" || result FAIL validate "$VALIDATE_ONLY failed validation" 5
    result PASS validate "$VALIDATE_ONLY ($(stat -c %s "$VALIDATE_ONLY") bytes)" 0
fi

# ------------------------------------------------------------------------------ prerequisites for a real build
missing=(); need() { command -v "$1" >/dev/null 2>&1 || missing+=("$1"); }
need ninja; need python3; need sha256sum; need mke2fs
if [ "$BOOTLOADER" != qemu ]; then need parted; need losetup; fi
[ "$BOOTLOADER" = limine ] && { need git; need mkfs.vfat; }
[ "$BOOTLOADER" = grub ] && need grub-install
[ ${#missing[@]} -gt 0 ] && result BLOCKED host-tools "missing: ${missing[*]}" 2
if [ "$(id -u)" != 0 ] && ! command -v sudo >/dev/null 2>&1 && ! command -v fuse2fs >/dev/null 2>&1; then
    result BLOCKED privileges "need root, sudo, or fuse2fs to build an image (Meta/build-image-*.sh mount filesystems)" 2
fi
[ -f "$BUILD_DIR/build.ninja" ] || result BLOCKED prerequisites "$BUILD_DIR is not configured; run Meta/exile.sh build $ARCH GNU first" 3
KERNEL="$BUILD_DIR/Kernel/Kernel"
[ -s "$KERNEL" ] || result BLOCKED prerequisites "kernel not built ($KERNEL); run Meta/exile.sh build $ARCH GNU first" 3
[ -d "$BUILD_DIR/Root/bin" ] || result BLOCKED prerequisites "userland not installed ($BUILD_DIR/Root/bin missing); run: ninja -C $BUILD_DIR install" 3

mkdir -p "$OUT" || result FAIL setup "cannot create $OUT" 4
LOG="$OUT/image-build.log"; : > "$LOG"
echo "== building target $TARGET in $BUILD_DIR" | tee -a "$LOG"
if ! ninja -j "$JOBS" -C "$BUILD_DIR" "$TARGET" >>"$LOG" 2>&1; then
    echo "---- last 40 lines of $LOG ----" >&2; tail -n 40 "$LOG" >&2
    result FAIL image-build "ninja $TARGET failed (log: $LOG)" 4
fi

IMAGE="$BUILD_DIR/$IMAGE_NAME"
[ -f "$IMAGE" ] || result FAIL image-build "target succeeded but $IMAGE does not exist" 4
validate_image "$IMAGE" "$KERNEL" 2>&1 | tee -a "$LOG"
[ "${PIPESTATUS[0]}" -eq 0 ] || result FAIL validate "$IMAGE failed validation (log: $LOG)" 5

( cd "$BUILD_DIR" && sha256sum "$IMAGE_NAME" ) > "$OUT/$IMAGE_NAME.sha256"
{
    echo "image   : $IMAGE"
    echo "size    : $(stat -c %s "$IMAGE") bytes"
    echo "sha256  : $(cut -d' ' -f1 "$OUT/$IMAGE_NAME.sha256")"
    echo "kernel  : $KERNEL ($(sha256sum "$KERNEL" | cut -d' ' -f1))"
} | tee "$OUT/image-build-summary.txt"
result PASS image-build "$IMAGE" 0
