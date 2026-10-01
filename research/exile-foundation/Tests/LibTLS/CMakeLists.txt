set(TEST_SOURCES
    TestTLSCertificateParser.cpp
    TestTLSHandshake.cpp
)

foreach(source IN LISTS TEST_SOURCES)
    exile_test("${source}" LibTLS LIBS LibTLS LibCrypto)
endforeach()
