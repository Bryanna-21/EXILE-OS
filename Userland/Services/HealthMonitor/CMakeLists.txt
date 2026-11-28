exile_component(
    HealthMonitor
    REQUIRED
    TARGETS HealthMonitor
)

set(SOURCES
    main.cpp
    HealthMonitor.cpp
)

exile_bin(HealthMonitor)
target_link_libraries(HealthMonitor PRIVATE LibCore LibMain)
