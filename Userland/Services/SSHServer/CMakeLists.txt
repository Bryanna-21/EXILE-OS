exile_component(
    SSHServer
    TARGETS SSHServer
)

set(SOURCES
    main.cpp
    ServerConfiguration.cpp
    SSHClient.cpp
    TCPClient.cpp
)

exile_bin(SSHServer)
target_link_libraries(SSHServer PRIVATE LibCore LibCrypto LibMain LibSSH)
