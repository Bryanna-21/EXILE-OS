set(TEST_SOURCES
    Regex.cpp
    RegexLibC.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibRegex LIBS LibRegex)
endforeach()
