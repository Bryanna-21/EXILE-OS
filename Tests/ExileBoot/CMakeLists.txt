# The verifier is plain freestanding C++ shared with the prekernel; compile it into the test directly.
set(TEST_SOURCES
    TestBootGuardian.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" ExileBoot LIBS LibCrypto)
    get_filename_component(test_name ${source} NAME_WE)
    target_sources(${test_name} PRIVATE ${ExileOS_SOURCE_DIR}/Kernel/Boot/Exile/BootGuardian.cpp)
endforeach()

# End-to-end tamper tests for the signing tool. They only run in the host (Lagom) build, which is where
# the tool is built and where ctest is used; the target build has no host shell to run them in.
if (COMMAND lagom_test AND TARGET ExileBootManifest)
    find_program(EXILE_BOOT_TEST_BASH bash REQUIRED)
    add_test(
        NAME ExileBootTamperTests
        COMMAND ${EXILE_BOOT_TEST_BASH} ${CMAKE_CURRENT_SOURCE_DIR}/test-boot-integrity.sh $<TARGET_FILE:ExileBootManifest>
    )
    set_tests_properties(ExileBootTamperTests PROPERTIES TIMEOUT 300)
endif()
