set(SOURCES
    Answer.cpp
    Name.cpp
    Packet.cpp
)

exile_lib(LibDNS dns)
target_link_libraries(LibDNS PRIVATE LibIPC)
