cmake_minimum_required(VERSION 3.22)

if (NOT DEFINED EXILE_ARCH OR NOT DEFINED EXILE_SYSROOT)
    message(FATAL_ERROR "EXILE_ARCH and EXILE_SYSROOT must be defined")
endif()

# EXILE_ARCH is used by the included file.
include(Userland/Libraries/LibC/Headers.cmake)

link_libc_headers("${CMAKE_CURRENT_SOURCE_DIR}/Userland/Libraries/LibC" "${EXILE_SYSROOT}/usr/include")
