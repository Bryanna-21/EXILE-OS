# Build-time checks for ENABLE_BOOT_VERIFICATION that live outside Kernel/.
#
# The prekernel build (Kernel/EFIPrekernel/CMakeLists.txt) signs the kernel image with BOOT_SIGNING_SEED and compiles
# BOOT_TRUST_ANCHOR_PUBKEY into the prekernel. It does not check that the two belong together, so a mismatched pair
# produces a prekernel that rejects its own kernel at boot. These checks catch that before the prekernel is built.
#
# Included from the top-level CMakeLists.txt after add_subdirectory(Kernel), when ENABLE_BOOT_VERIFICATION is ON.
# See Documentation/BootIntegrity.md.

if (NOT ENABLE_BOOT_VERIFICATION OR NOT TARGET EFIPrekernel)
    return()
endif()

# A signing seed inside the source tree is one `git add .` away from being published.
get_filename_component(exile_boot_seed_real "${BOOT_SIGNING_SEED}" REALPATH)
get_filename_component(exile_boot_source_real "${ExileOS_SOURCE_DIR}" REALPATH)
cmake_path(IS_PREFIX exile_boot_source_real "${exile_boot_seed_real}" NORMALIZE exile_boot_seed_in_source)
if (exile_boot_seed_in_source)
    message(FATAL_ERROR
        "BOOT_SIGNING_SEED must not be inside the source tree (${exile_boot_seed_real}).\n"
        "Keep the signing seed offline or in a secret store outside the repository checkout.")
endif()

# Runs on every build, before EFIPrekernel (and so before the manifest is signed) is built, and fails the build if the
# seed does not derive the trust anchor.
add_custom_target(ExileBootKeyCheck
    COMMAND $<TARGET_FILE:Lagom::ExileBootManifest> check-keys "${BOOT_SIGNING_SEED}" "${BOOT_TRUST_ANCHOR_PUBKEY}"
    DEPENDS Lagom::ExileBootManifest
    COMMENT "Checking that BOOT_SIGNING_SEED matches BOOT_TRUST_ANCHOR_PUBKEY"
    VERBATIM
)
add_dependencies(EFIPrekernel ExileBootKeyCheck)
