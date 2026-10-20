# The verifier is plain freestanding C++ shared with the prekernel; compile it into the test directly.
set(TEST_SOURCES
    TestBootGuardian.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" ExileBoot LIBS LibCrypto)
    get_filename_component(test_name ${source} NAME_WE)
    target_sources(${test_name} PRIVATE ${ExileOS_SOURCE_DIR}/Kernel/Boot/Exile/BootGuardian.cpp)
endforeach()
