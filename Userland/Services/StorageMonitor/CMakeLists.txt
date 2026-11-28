exile_component(
    StorageMonitor
    REQUIRED
    TARGETS StorageMonitor
)

set(SOURCES
    main.cpp
    StorageMonitor.cpp
)

exile_bin(StorageMonitor)
target_link_libraries(StorageMonitor PRIVATE LibCore LibMain)
