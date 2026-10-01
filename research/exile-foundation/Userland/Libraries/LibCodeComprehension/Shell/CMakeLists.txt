set(SOURCES
    ShellComprehensionEngine.cpp
)

exile_lib(LibShellComprehension shellcomprehension)
target_link_libraries(LibShellComprehension PRIVATE LibCodeComprehension)
