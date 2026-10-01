set(SOURCES
    Document.cpp
    GeminiRequest.cpp
    GeminiResponse.cpp
    Job.cpp
    Line.cpp
)

exile_lib(LibGemini gemini)
target_link_libraries(LibGemini PRIVATE LibCore LibTLS LibURL)
