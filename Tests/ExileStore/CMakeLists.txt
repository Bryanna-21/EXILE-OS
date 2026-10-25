set(TEST_SOURCES
    TestCatalogLoader.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" ExileStore)
    target_sources(TestCatalogLoader PRIVATE ${ExileOS_SOURCE_DIR}/Userland/Applications/ExileStore/src/CatalogLoader.cpp)
    target_include_directories(TestCatalogLoader PRIVATE ${ExileOS_SOURCE_DIR}/Userland/Applications/ExileStore/src)
endforeach()
