set(SOURCES
    Brotli.cpp
    BrotliDictionary.cpp
    Deflate.cpp
    Lzma.cpp
    Lzma2.cpp
    PackBitsDecoder.cpp
    Xz.cpp
    Zlib.cpp
    Gzip.cpp
)

exile_lib(LibCompress compress)
target_link_libraries(LibCompress PRIVATE LibCore LibCrypto)
