exile_component(
    SystemLog
    REQUIRED
    TARGETS SystemLog
)

set(SOURCES
    main.cpp
)

exile_bin(SystemLog)
target_link_libraries(SystemLog PRIVATE LibCore LibMain)
