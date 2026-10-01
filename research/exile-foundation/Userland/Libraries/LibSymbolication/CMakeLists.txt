set(SOURCES
    Symbolication.cpp
)

exile_lib(LibSymbolication symbolication)
target_link_libraries(LibSymbolication PRIVATE LibCore LibDebug LibELF LibFileSystem)
