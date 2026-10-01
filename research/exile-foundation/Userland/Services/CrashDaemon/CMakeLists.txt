exile_component(
    CrashDaemon
    REQUIRED
    TARGETS CrashDaemon
)

set(SOURCES
    main.cpp
)

exile_bin(CrashDaemon)
target_link_libraries(CrashDaemon PRIVATE LibCompress LibCore LibCoredump LibMain)
