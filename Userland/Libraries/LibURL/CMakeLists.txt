set(SOURCES
    Origin.cpp
    Parser.cpp
    URL.cpp
)

exile_lib(LibURL url)
target_link_libraries(LibURL PRIVATE LibUnicode LibTextCodec)
