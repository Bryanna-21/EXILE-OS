set(PREKERNEL_KERNEL_IMAGE_PATH ${CMAKE_CURRENT_BINARY_DIR}/../Kernel_shared_object)

# Optional signed boot manifest (see Documentation/BootIntegrity.md). Off by default.
set(PREKERNEL_BOOT_MANIFEST_ASM "")
set(PREKERNEL_BOOT_MANIFEST_PATH ${CMAKE_CURRENT_BINARY_DIR}/kernel.exbm)
if (ENABLE_BOOT_VERIFICATION)
    if ("${BOOT_SIGNING_SEED}" STREQUAL "" OR "${BOOT_TRUST_ANCHOR_PUBKEY}" STREQUAL "")
        message(FATAL_ERROR
            "ENABLE_BOOT_VERIFICATION requires BOOT_SIGNING_SEED and BOOT_TRUST_ANCHOR_PUBKEY.\n"
            "Create a key pair with: ExileBootManifest keygen <seed-file> <public-key-file>")
    endif()
    foreach(key_file IN ITEMS "${BOOT_SIGNING_SEED}" "${BOOT_TRUST_ANCHOR_PUBKEY}")
        if (NOT EXISTS "${key_file}")
            message(FATAL_ERROR "Boot key file does not exist: ${key_file}")
        endif()
        file(SIZE "${key_file}" key_file_size)
        if (NOT key_file_size EQUAL 32)
            message(FATAL_ERROR "Boot key file must be exactly 32 raw bytes, got ${key_file_size}: ${key_file}")
        endif()
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${BOOT_TRUST_ANCHOR_PUBKEY}")

    file(READ "${BOOT_TRUST_ANCHOR_PUBKEY}" boot_trust_anchor_hex HEX)
    string(REGEX REPLACE "([0-9a-f][0-9a-f])" "0x\\1, " EXILE_BOOT_TRUST_ANCHOR_BYTES "${boot_trust_anchor_hex}")
    configure_file(TrustAnchors.cpp.in TrustAnchors.cpp @ONLY)

    # Sign the very file that KernelImage.S embeds, so the manifest always describes the bytes that will be loaded.
    add_custom_command(
        OUTPUT ${PREKERNEL_BOOT_MANIFEST_PATH}
        COMMAND $<TARGET_FILE:Lagom::ExileBootManifest> sign ${PREKERNEL_KERNEL_IMAGE_PATH} "${BOOT_SIGNING_SEED}" ${PREKERNEL_BOOT_MANIFEST_PATH}
        DEPENDS Kernel ${PREKERNEL_KERNEL_IMAGE_PATH} Lagom::ExileBootManifest
        VERBATIM
    )
    set(PREKERNEL_BOOT_MANIFEST_ASM ".section .rodata
.global start_of_boot_manifest
.global end_of_boot_manifest
.p2align 3
start_of_boot_manifest:
.incbin \"${PREKERNEL_BOOT_MANIFEST_PATH}\"
end_of_boot_manifest:")
endif()

configure_file(KernelImage.S.in KernelImage.S @ONLY)

set(SOURCES
    init.cpp

    ConfigurationTable.cpp
    DebugOutput.cpp
    Filesystem.cpp
    GOP.cpp
    Panic.cpp
    Relocation.cpp
    Runtime.cpp
    kmalloc.cpp

    KernelImage.S

    Arch/${EXILE_ARCH}/Boot.cpp
    Arch/${EXILE_ARCH}/MMU.cpp

    ../Firmware/EFI/EFI.cpp
    ../Library/MiniStdLib.cpp
    ../Prekernel/UBSanitizer.cpp

    ../../Userland/Libraries/LibELF/Image.cpp
    ../../Userland/Libraries/LibELF/Validation.cpp

    ../../AK/Format.cpp
    ../../AK/StringBuilder.cpp
    ../../AK/StringUtils.cpp
    ../../AK/StringView.cpp
)
# UBSAN handlers should not get sanitized themselves
set_source_files_properties(../Prekernel/UBSanitizer.cpp PROPERTIES COMPILE_FLAGS "-fno-sanitize=undefined")

if ("${EXILE_ARCH}" STREQUAL "x86_64")
    set(SOURCES
        ${SOURCES}
        Arch/x86_64/CPUID.cpp
    )
endif()

if (ENABLE_BOOT_VERIFICATION)
    set_source_files_properties(KernelImage.S PROPERTIES OBJECT_DEPENDS "${PREKERNEL_KERNEL_IMAGE_PATH};${PREKERNEL_BOOT_MANIFEST_PATH}")
else()
    set_source_files_properties(KernelImage.S PROPERTIES OBJECT_DEPENDS ${PREKERNEL_KERNEL_IMAGE_PATH})
endif()

add_compile_definitions(PREKERNEL)

add_executable(EFIPrekernel ${SOURCES})

if (ENABLE_BOOT_VERIFICATION)
    target_sources(EFIPrekernel PRIVATE
        BootVerification.cpp
        ${CMAKE_CURRENT_BINARY_DIR}/TrustAnchors.cpp
        ../Boot/Exile/BootGuardian.cpp
        ../../Userland/Libraries/LibCrypto/Curves/Curve25519.cpp
        ../../Userland/Libraries/LibCrypto/Curves/Ed25519.cpp
        ../../Userland/Libraries/LibCrypto/Hash/SHA2.cpp
    )
    target_compile_definitions(EFIPrekernel PRIVATE EXILE_BOOT_VERIFICATION)
endif()

add_dependencies(EFIPrekernel Kernel)

target_compile_options(EFIPrekernel PRIVATE -fno-threadsafe-statics)
target_link_options(EFIPrekernel PRIVATE LINKER:-T ${CMAKE_CURRENT_SOURCE_DIR}/linker.ld LINKER:--no-dynamic-linker -nostdlib)
set_target_properties(EFIPrekernel PROPERTIES LINK_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/linker.ld)

if (CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    target_link_libraries(EFIPrekernel PRIVATE gcc)

    # Prevent naively implemented string functions (like strlen) from being "optimized" into a call to themselves.
    set_source_files_properties(../Library/MiniStdLib.cpp
        PROPERTIES COMPILE_FLAGS "-fno-tree-loop-distribution -fno-tree-loop-distribute-patterns")
elseif(CMAKE_CXX_COMPILER_ID MATCHES "Clang$")
    target_link_libraries(EFIPrekernel PRIVATE clang_rt.builtins)
endif()

# The PrekernelPEImageGenerator doesn't support RELR relocations.
if (CMAKE_CXX_COMPILER_ID MATCHES "Clang$")
    target_link_options(EFIPrekernel PRIVATE LINKER:-z,nopack-relative-relocs LINKER:--pack-dyn-relocs=none)
elseif("${EXILE_ARCH}" STREQUAL "x86_64")
    target_link_options(EFIPrekernel PRIVATE LINKER:-z,nopack-relative-relocs)
endif()

add_custom_command(
    OUTPUT ${CMAKE_CURRENT_BINARY_DIR}/Kernel.efi
    COMMAND $<TARGET_FILE:Lagom::PrekernelPEImageGenerator> ${CMAKE_CURRENT_BINARY_DIR}/EFIPrekernel ${CMAKE_CURRENT_BINARY_DIR}/Kernel.efi
    VERBATIM
    DEPENDS EFIPrekernel Lagom::PrekernelPEImageGenerator
)

add_custom_target(generate_kernel_efi_image ALL DEPENDS ${CMAKE_CURRENT_BINARY_DIR}/Kernel.efi)

install(FILES ${CMAKE_CURRENT_BINARY_DIR}/Kernel.efi DESTINATION boot)

# Remove options which the Prekernel environment doesn't support.
get_target_property(EFI_PREKERNEL_COMPILE_OPTIONS EFIPrekernel COMPILE_OPTIONS)
list(REMOVE_ITEM EFI_PREKERNEL_COMPILE_OPTIONS "-fsanitize-coverage=trace-pc")
list(REMOVE_ITEM EFI_PREKERNEL_COMPILE_OPTIONS "-fsanitize=kernel-address")
set_target_properties(EFIPrekernel PROPERTIES COMPILE_OPTIONS "${EFI_PREKERNEL_COMPILE_OPTIONS}")
