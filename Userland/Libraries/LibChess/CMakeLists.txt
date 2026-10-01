set(SOURCES
    Chess.cpp
    UCICommand.cpp
    UCIEndpoint.cpp
)

exile_lib(LibChess chess)
target_link_libraries(LibChess PRIVATE LibCore)
