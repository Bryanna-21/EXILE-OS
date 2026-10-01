set(TEST_SOURCES
    TestHID.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibHID LIBS LibHID)
endforeach()
