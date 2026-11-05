set(TEST_SOURCES
    TestWebViewURL.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibWebView LIBS LibWebView LibURL)
endforeach()
