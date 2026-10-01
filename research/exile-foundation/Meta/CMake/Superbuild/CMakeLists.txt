cmake_minimum_required(VERSION 3.20)

project(
    ExileSuperbuild
    DESCRIPTION "Orchestrate host and target builds in a single Exile OS build"
    LANGUAGES NONE
)

# NOTE: Before CMake 3.19, if a custom command is attached to multiple step targets for Makefile and Visual Studio generators,
#       it might be run multiple times during the build. Enable new behavior of policy CMP0114 to avoid this.
cmake_policy(SET CMP0114 NEW)

get_filename_component(
    EXILE_SOURCE_DIR "${PROJECT_SOURCE_DIR}/../../.."
    ABSOLUTE CACHE
)
set(EXILE_ARCH "x86_64" CACHE STRING "Target architecture for Exile OS.")
set(EXILE_TOOLCHAIN "GNU" CACHE STRING "Compiler toolchain to use for Exile OS (GNU or Clang)")

# FIXME: It is preferred to keep all the sub-build artifacts below the binary directory for the superbuild
#        However, this has an impact on developer's IDE settings and more significantly, the Ports tree.
#        See the upstream build-system discussion that informed this configuration#discussion_r697877603
set(EXILE_BUILD_DIR_SUFFIX "")
if(NOT EXILE_TOOLCHAIN STREQUAL "GNU")
  string(TOLOWER "${EXILE_TOOLCHAIN}" EXILE_BUILD_DIR_SUFFIX)
endif()
list(APPEND lagom_options "-DCMAKE_LIBRARY_PATH:STRING=C:/msys64/mingw64/lib")
set(EXILE_BUILD_DIR "${PROJECT_BINARY_DIR}/../${EXILE_ARCH}${EXILE_BUILD_DIR_SUFFIX}")

# Pkgconf incorrectly discards a sysroot if it doesn't match the start of the path to the
# library file. To avoid that, resolve our sysroot into an absolute and canonical path
# that matches pkgconf's result for resolving the library file.
get_filename_component(EXILE_BUILD_DIR "${EXILE_BUILD_DIR}" ABSOLUTE)

# TODO: Figure out if and how we can skip this when building on Exile OS.
configure_file("${EXILE_SOURCE_DIR}/Toolchain/CMake/${EXILE_TOOLCHAIN}Toolchain.txt.in" "${EXILE_BUILD_DIR}/CMakeToolchain.txt" @ONLY)
set(EXILE_TOOLCHAIN_FILE "${EXILE_BUILD_DIR}/CMakeToolchain.txt" CACHE PATH "Toolchain file to use for cross-compilation")
# Support non-cross builds by stuffing this in a variable
set(EXILE_TOOLCHAIN_FILE_ARG "-DCMAKE_TOOLCHAIN_FILE:STRING=${EXILE_TOOLCHAIN_FILE}")

configure_file("${EXILE_SOURCE_DIR}/Toolchain/CMake/meson-cross-file-${EXILE_TOOLCHAIN}.txt.in" "${EXILE_BUILD_DIR}/meson-cross-file.txt" @ONLY)

# Allow the Ninja generators to output messages as they happen by assigning
# these jobs to the 'console' job pool
set(console_access "")
if(CMAKE_GENERATOR MATCHES "^Ninja")
  set(
      console_access
      USES_TERMINAL_CONFIGURE YES
      USES_TERMINAL_BUILD YES
      USES_TERMINAL_INSTALL YES
  )
endif()

include(ExternalProject)

# Collect options for Lagom build
set(lagom_options "")
macro(exile_option name)
    set(${ARGV})
    get_property(type CACHE "${name}" PROPERTY TYPE)
    list(APPEND lagom_options "-D${name}:${type}=${${name}}")
endmacro()
include("${EXILE_SOURCE_DIR}/Meta/CMake/lagom_options.cmake")

# Forward user defined host toolchain to lagom build
if (DEFINED CMAKE_C_COMPILER)
    set(CMAKE_C_COMPILER "${CMAKE_C_COMPILER}" CACHE STRING "C Compiler to use for host builds")
    list(APPEND lagom_options "-DCMAKE_C_COMPILER:STRING=${CMAKE_C_COMPILER}")
endif()
if (DEFINED CMAKE_CXX_COMPILER)
    set(CMAKE_CXX_COMPILER "${CMAKE_CXX_COMPILER}" CACHE STRING "C++ Compiler to use for host builds")
    list(APPEND lagom_options "-DCMAKE_CXX_COMPILER:STRING=${CMAKE_CXX_COMPILER}")
endif()
list(APPEND lagom_options "-DCMAKE_LIBRARY_PATH:STRING=C:/msys64/mingw64/lib")
list(APPEND lagom_options "-DCMAKE_INCLUDE_PATH:STRING=C:/msys64/mingw64/include")

ExternalProject_Add(
    lagom
    SOURCE_DIR "${EXILE_SOURCE_DIR}/Meta/Lagom"
    BINARY_DIR "${PROJECT_BINARY_DIR}/../lagom"
    INSTALL_DIR "${PROJECT_BINARY_DIR}/../lagom-install"
    EXCLUDE_FROM_ALL YES
    CMAKE_CACHE_ARGS
    "-DCMAKE_INSTALL_PREFIX:STRING=<INSTALL_DIR>"
    "-DEXILE_CACHE_DIR:STRING=${EXILE_CACHE_DIR}"
    ${lagom_options}
    # Always call the build step of tools, so keeping things up-to-date is easy
    BUILD_ALWAYS YES
    # Expose install step as a target, so it can be depended on
    STEP_TARGETS install
    ${console_access}
)

# Collect options for Exile OS build
set(exile_options "")
macro(exile_option name)
    set(${ARGV})
    get_property(type CACHE "${name}" PROPERTY TYPE)
    list(APPEND exile_options "-D${name}:${type}=${${name}}")
endmacro()
include("${EXILE_SOURCE_DIR}/Meta/CMake/exile_options.cmake")

ExternalProject_Add(
    exile
    SOURCE_DIR "${EXILE_SOURCE_DIR}"
    BINARY_DIR "${EXILE_BUILD_DIR}"
    CMAKE_CACHE_ARGS
    # Tell the find_package(Lagom REQUIRED) command call where to find
    # the CMake package
    "-DCMAKE_PREFIX_PATH:STRING=${PROJECT_BINARY_DIR}/../lagom-install"
    "-DEXILE_CACHE_DIR:STRING=${EXILE_CACHE_DIR}"
    "-DEXILE_ARCH:STRING=${EXILE_ARCH}"
    "${EXILE_TOOLCHAIN_FILE_ARG}"
    ${exile_options}
    # Always call the build step
    BUILD_ALWAYS YES
    # Host tools must be built and installed before the OS can be built
    DEPENDS lagom-install
    STEP_TARGETS configure install
    ${console_access}
)
