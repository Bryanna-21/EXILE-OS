set(SOURCES
    CharacterMap.cpp
    CharacterMapFile.cpp
)

exile_lib(LibKeyboard keyboard)
target_link_libraries(LibKeyboard PRIVATE LibCore)
