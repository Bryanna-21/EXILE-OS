set(TEST_SOURCES
    TestEmoji.cpp
    TestIDNA.cpp
    TestPunycode.cpp
    TestSegmentation.cpp
    TestUnicodeCharacterTypes.cpp
    TestUnicodeNormalization.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibUnicode LIBS LibUnicode)

    get_filename_component(target "${source}" NAME_WLE)
endforeach()
