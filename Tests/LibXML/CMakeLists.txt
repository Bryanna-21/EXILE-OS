set(TEST_SOURCES
    TestParser.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibXML LIBS LibXML)
endforeach()
