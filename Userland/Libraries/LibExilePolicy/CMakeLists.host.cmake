cmake_minimum_required(VERSION 3.16)
project(LibExilePolicy VERSION 0.1.0 LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# SHA-256 is reused from ExileStore rather than duplicated.
set(EXILE_STORE_CORE "${CMAKE_CURRENT_SOURCE_DIR}/../../Applications/ExileStore/src/core" CACHE PATH "ExileStore src/core")
set(EXILE_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../.." CACHE PATH "repository root")

add_library(exile_policy STATIC permission.cpp broker.cpp kernel_map.cpp ${EXILE_STORE_CORE}/sha256.cpp ${EXILE_STORE_CORE}/util.cpp)
target_include_directories(exile_policy PUBLIC . ${EXILE_STORE_CORE})
target_compile_options(exile_policy PRIVATE -Wall -Wextra -Wpedantic)

enable_testing()
add_executable(exile_policy_tests tests/test_main.cpp)
target_link_libraries(exile_policy_tests exile_policy)
target_compile_definitions(exile_policy_tests PRIVATE EXILE_REPO_ROOT="${EXILE_REPO_ROOT}")
target_compile_options(exile_policy_tests PRIVATE -Wall -Wextra)
add_test(NAME exile_policy_tests COMMAND exile_policy_tests)
