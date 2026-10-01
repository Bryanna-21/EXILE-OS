set(SOURCES
    Node.cpp
    PageNode.cpp
    Path.cpp
    SectionNode.cpp
    SubsectionNode.cpp
)

exile_lib(LibManual manual)
target_link_libraries(LibManual PRIVATE LibCore LibFileSystem LibURL)
