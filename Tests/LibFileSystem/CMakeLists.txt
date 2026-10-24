set(TEST_SOURCES
    TestTrash.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibFileSystem)
endforeach()
