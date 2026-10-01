exile_component(
    Debugger
    TARGETS Debugger
)

set(SOURCES
    main.cpp
)

exile_bin(Debugger)
target_link_libraries(Debugger PRIVATE LibCore LibDebug LibELF LibLine LibMain LibDisassembly)
