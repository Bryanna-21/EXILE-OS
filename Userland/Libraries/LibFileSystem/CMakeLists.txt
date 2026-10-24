set(SOURCES
    FileSystem.cpp
    TempFile.cpp
    Trash.cpp
)

exile_lib(LibFileSystem filesystem)
target_link_libraries(LibFileSystem PRIVATE LibCoreMinimal)
