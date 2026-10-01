set(SOURCES
    CMakeCache/Lexer.cpp
    CMakeCache/SyntaxHighlighter.cpp
    Lexer.cpp
    SyntaxHighlighter.cpp
    Token.cpp
)

exile_lib(LibCMake cmake)
target_link_libraries(LibCMake PRIVATE LibSyntax)
