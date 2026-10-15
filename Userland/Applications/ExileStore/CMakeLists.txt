cmake_minimum_required(VERSION 3.16)
project(ExileStore VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(EXILE_STORE_WITH_OPENSSL "Use OpenSSL for Ed25519 signature verification" ON)
option(EXILE_STORE_WERROR "Treat warnings as errors" OFF)

find_package(Threads REQUIRED)

set(EXILE_CORE_SOURCES
    src/core/util.cpp
    src/core/json.cpp
    src/core/version.cpp
    src/core/sha256.cpp
    src/core/model.cpp
    src/core/catalog.cpp
    src/core/device.cpp
    src/core/search.cpp
    src/core/trust.cpp
    src/core/transport.cpp
    src/core/library.cpp
    src/core/backend.cpp
    src/core/appstate.cpp
    src/core/installer.cpp
    src/core/catalogclient.cpp
    src/core/expkg.cpp
    src/ui/text.cpp
    src/ui/render.cpp
    src/ui/controller.cpp
)

add_library(exile_core STATIC ${EXILE_CORE_SOURCES})
target_include_directories(exile_core PUBLIC src)
target_link_libraries(exile_core PUBLIC Threads::Threads)

if(EXILE_STORE_WITH_OPENSSL)
  find_package(OpenSSL QUIET)
  if(OpenSSL_FOUND)
    target_compile_definitions(exile_core PUBLIC EXILE_HAS_OPENSSL=1)
    target_link_libraries(exile_core PUBLIC OpenSSL::Crypto)
    message(STATUS "Exile Store: Ed25519 verification via OpenSSL ${OPENSSL_VERSION}")
  else()
    message(WARNING "OpenSSL not found: signature verification unavailable, installs will be refused")
  endif()
endif()

if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
  target_compile_options(exile_core PUBLIC -Wall -Wextra -Wpedantic)
  if(EXILE_STORE_WERROR)
    target_compile_options(exile_core PUBLIC -Werror)
  endif()
endif()

add_executable(exile-store src/main.cpp)
target_link_libraries(exile-store PRIVATE exile_core)

add_executable(expkg-tool tools/expkg_tool.cpp)
target_link_libraries(expkg-tool PRIVATE exile_core)

enable_testing()
add_executable(exile_tests
    tests/test_main.cpp
    tests/test_util_json.cpp
    tests/test_version_sha.cpp
    tests/test_catalog.cpp
    tests/test_compat_search.cpp
    tests/test_trust.cpp
    tests/test_expkg_backend.cpp
    tests/test_installer.cpp
    tests/test_ui.cpp
)
target_link_libraries(exile_tests PRIVATE exile_core)
add_test(NAME exile_tests COMMAND exile_tests)
if(NOT WIN32 AND EXILE_STORE_WITH_OPENSSL AND OpenSSL_FOUND)
  add_test(NAME cli_smoke
           COMMAND sh ${CMAKE_SOURCE_DIR}/tools/smoke_test.sh $<TARGET_FILE:exile-store> $<TARGET_FILE:expkg-tool>
                   ${CMAKE_BINARY_DIR}/smoke-work)
endif()

install(TARGETS exile-store expkg-tool RUNTIME DESTINATION bin)
install(DIRECTORY data/ DESTINATION share/exile-store)
