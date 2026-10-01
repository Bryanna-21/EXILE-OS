exile_component(
    DeviceMapper
    REQUIRED
    TARGETS DeviceMapper
)

set(SOURCES
    main.cpp
    DeviceEventLoop.cpp
)

exile_bin(DeviceMapper)
target_link_libraries(DeviceMapper PRIVATE LibCore LibFileSystem LibMain)
