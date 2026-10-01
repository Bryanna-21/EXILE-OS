set(SOURCES
    Client.cpp
    Objects.cpp
    Parser.cpp
    QuotedPrintable.cpp
    MessageHeaderEncoding.cpp
)

set(GENERATED_SOURCES)

exile_lib(LibIMAP imap)
target_link_libraries(LibIMAP PRIVATE LibCore LibCrypto LibTextCodec LibTLS)
