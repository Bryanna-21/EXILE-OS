exile_component(
    TelnetServer
    TARGETS TelnetServer
)

set(SOURCES
    Client.cpp
    main.cpp
    Parser.cpp
)

exile_bin(TelnetServer)
target_link_libraries(TelnetServer PRIVATE LibCore LibMain)
