set(TEST_SOURCES
    TestMetrics.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibLine LIBS LibLine LibUnicode)
endforeach()
