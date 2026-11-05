set(SOURCES
    Image.cpp
    Validation.cpp
)

exile_install_headers("LibELF")
exile_install_sources("Userland/Libraries/LibELF")

if (SERENITYOS)
    list(APPEND SOURCES
        Arch/${EXILE_ARCH}/entry.S
        Arch/${EXILE_ARCH}/plt_trampoline.S
        Arch/${EXILE_ARCH}/tls.cpp
        DynamicLinker.cpp
        DynamicLoader.cpp
        DynamicObject.cpp
        ELFBuild.cpp
        Relocation.cpp
    )

    if (EXILE_ARCH STREQUAL "aarch64")
        list(APPEND SOURCES
            Arch/aarch64/tls.S
        )
    elseif (EXILE_ARCH STREQUAL "riscv64")
        list(APPEND SOURCES
            Arch/riscv64/ExtensionBitmask.cpp
        )
    endif()

    add_library(DynamicLoader_LibELF STATIC ${SOURCES})
    target_link_libraries(DynamicLoader_LibELF
        PUBLIC DynamicLoader_CompileOptions
        PRIVATE DynamicLoader_LibC
    )
endif()

exile_lib(LibELF elf ${SOURCES})
target_link_libraries(LibELF PRIVATE LibSystem)
