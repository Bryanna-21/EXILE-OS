set(TEST_SOURCES
    TestQuotedPrintable.cpp
    TestMessageHeaderEncoding.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibIMAP LIBS LibIMAP)
endforeach()
