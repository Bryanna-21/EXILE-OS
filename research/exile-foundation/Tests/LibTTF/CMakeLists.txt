set(TEST_SOURCES
    TestCmap.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibTTF LIBS LibGfx)
endforeach()
