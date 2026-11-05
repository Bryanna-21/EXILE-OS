exile_component(
    SystemServer
    REQUIRED
    TARGETS SystemServer
)

set(SOURCES
    main.cpp
    Service.cpp
)

exile_bin(SystemServer)
target_link_libraries(SystemServer PRIVATE LibCore LibFileSystem LibMain)
