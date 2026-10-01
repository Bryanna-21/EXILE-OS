set(TEST_SOURCES
    test-parser.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibGLSL LIBS LibGLSL)
endforeach()
