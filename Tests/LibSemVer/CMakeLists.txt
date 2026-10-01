set(TEST_SOURCES
    TestFromStringView.cpp
    TestSemVer.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibSemVer LIBS LibSemVer)
endforeach()
