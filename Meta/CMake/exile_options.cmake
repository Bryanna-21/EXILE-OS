#
# Options specific to the Exile OS (target) build
#

include(${CMAKE_CURRENT_LIST_DIR}/common_options.cmake NO_POLICY_SCOPE)

exile_option(ENABLE_PCI_IDS_DOWNLOAD ON CACHE BOOL "Enable download of the pci.ids database at build time")
exile_option(ENABLE_USB_IDS_DOWNLOAD ON CACHE BOOL "Enable download of the usb.ids database at build time")
exile_option(ENABLE_PNP_IDS_DOWNLOAD ON CACHE BOOL "Enable download of the pnp.ids database at build time")
exile_option(ENABLE_RASPBERRY_PI_DTB_DOWNLOAD ON CACHE BOOL "Enable download of the Raspberry Pi 3/4 devicetree blob at build time")
exile_option(ENABLE_KERNEL_ADDRESS_SANITIZER OFF CACHE BOOL "Enable kernel address sanitizer testing in gcc/clang")
exile_option(ENABLE_KERNEL_COVERAGE_COLLECTION  OFF CACHE BOOL "Enable KCOV and kernel coverage instrumentation in gcc/clang")
exile_option(ENABLE_KERNEL_COVERAGE_COLLECTION_DEBUG  OFF CACHE BOOL "Enable KCOV and kernel coverage instrumentation debugging")
exile_option(ENABLE_KERNEL_LTO OFF CACHE BOOL "Build the kernel with link-time optimization")
exile_option(ENABLE_KERNEL_UNDEFINED_SANITIZER ON CACHE BOOL "Enable the Kernel Undefined Behavior Sanitizer (KUBSAN)")
exile_option(ENABLE_KERNEL_UNDEFINED_SANITIZER_ALWAYS_DEADLY ON CACHE BOOL "Make KUBSAN always deadly")
exile_option(ENABLE_EXTRA_KERNEL_DEBUG_SYMBOLS  OFF CACHE BOOL "Enable -Og and -ggdb3 options for Kernel code for easier debugging")
exile_option(ENABLE_MOLD_LINKER OFF CACHE BOOL "Link the Exile OS userland with the mold linker")
exile_option(ENABLE_USERSPACE_COVERAGE_COLLECTION OFF CACHE BOOL "Enable code coverage instrumentation for userspace binaries in clang")
exile_option(ENABLE_BOOT_VERIFICATION OFF CACHE BOOL "Verify a signed boot manifest in the EFI prekernel and refuse to boot a kernel image that fails verification (experimental, see Documentation/BootIntegrity.md)")
exile_option(BOOT_SIGNING_SEED "" CACHE FILEPATH "Raw 32-byte Ed25519 seed that signs the kernel boot manifest at build time. Keep it offline and never commit it. Required when ENABLE_BOOT_VERIFICATION is ON")
exile_option(BOOT_TRUST_ANCHOR_PUBKEY "" CACHE FILEPATH "Raw 32-byte Ed25519 public key compiled into the EFI prekernel as the boot trust anchor. Required when ENABLE_BOOT_VERIFICATION is ON")
