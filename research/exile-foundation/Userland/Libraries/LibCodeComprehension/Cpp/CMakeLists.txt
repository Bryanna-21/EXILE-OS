set(SOURCES
    CppComprehensionEngine.cpp
)

exile_lib(LibCppComprehension cppcomprehension)
target_link_libraries(LibCppComprehension PRIVATE LibCodeComprehension)

exile_component(
    CppComprehensionTests
    TARGETS CppComprehensionTests
)

set(SOURCES
    CppComprehensionEngine.cpp
    Tests.cpp
)

exile_bin(CppComprehensionTests)

target_link_libraries(CppComprehensionTests PRIVATE LibCodeComprehension LibCore LibCpp LibFileSystem LibRegex LibMain)
