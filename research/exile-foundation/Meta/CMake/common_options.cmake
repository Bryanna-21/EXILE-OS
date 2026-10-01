#
# Options common for the Exile OS (target) and Lagom (host) builds
#

# Make relative paths in depfiles be relative to CMAKE_CURRENT_BINARY_DIR rather than to CMAKE_BINARY_DIR
if (POLICY CMP0116)
    cmake_policy(SET CMP0116 NEW)
endif()

# Acknowledge that DESTINATION paths for install() commands will be normalized
if (POLICY CMP0177)
    cmake_policy(SET CMP0177 NEW)
endif()

exile_option(ENABLE_COMPILETIME_FORMAT_CHECK ON CACHE BOOL "Enable compiletime format string checks")
exile_option(ENABLE_UNDEFINED_SANITIZER OFF CACHE BOOL "Enable undefined behavior sanitizer testing in gcc/clang")
exile_option(UNDEFINED_BEHAVIOR_IS_FATAL OFF CACHE BOOL "Make undefined behavior sanitizer errors non-recoverable")

exile_option(ENABLE_ALL_THE_DEBUG_MACROS OFF CACHE BOOL "Enable all debug macros to validate they still compile")
exile_option(ENABLE_ALL_DEBUG_FACILITIES OFF CACHE BOOL "Enable all noisy debug symbols and options. Not recommended for normal developer use")
exile_option(ENABLE_ADOBE_ICC_PROFILES_DOWNLOAD ON CACHE BOOL "Enable download of Adobe's ICC profiles")
exile_option(ENABLE_COMPILETIME_HEADER_CHECK OFF CACHE BOOL "Enable compiletime check that each library header compiles stand-alone")

exile_option(ENABLE_TIME_ZONE_DATABASE_DOWNLOAD ON CACHE BOOL "Enable download of the IANA Time Zone Database at build time")
exile_option(ENABLE_UNICODE_DATABASE_DOWNLOAD ON CACHE BOOL "Enable download of Unicode UCD and CLDR files at build time")
exile_option(ENABLE_PUBLIC_SUFFIX_DOWNLOAD ON CACHE BOOL "Enable download of the Public Suffix List at build time")
exile_option(INCLUDE_WASM_SPEC_TESTS OFF CACHE BOOL "Download and include the WebAssembly spec testsuite")
exile_option(INCLUDE_FLAC_SPEC_TESTS OFF CACHE BOOL "Download and include the FLAC spec testsuite")
exile_option(ENABLE_CACERT_DOWNLOAD ON CACHE BOOL "Enable download of cacert.pem at build time")
exile_option(ENABLE_ACCELERATED_GRAPHICS ON CACHE BOOL "Enable use of accelerated graphics APIs")

exile_option(HACKSTUDIO_BUILD OFF CACHE BOOL "Automatically enabled when building from HackStudio")

exile_option(ENABLE_JAKT ON CACHE BOOL "Enable building jakt files")

exile_option(EXILE_CACHE_DIR "${PROJECT_BINARY_DIR}/../caches" CACHE PATH "Location of shared cache of downloaded files")
exile_option(ENABLE_NETWORK_DOWNLOADS ON CACHE BOOL "Allow downloads of required files. If OFF, required files must already be present in EXILE_CACHE_DIR")

exile_option(ENABLE_CLANG_PLUGINS OFF CACHE BOOL "Enable building with the Clang plugins")
exile_option(ENABLE_CLANG_PLUGINS_INVALID_FUNCTION_MEMBERS OFF CACHE BOOL "Enable detecting invalid function types as members of GC-allocated objects")
