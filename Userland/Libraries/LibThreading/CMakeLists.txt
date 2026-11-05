set(SOURCES
    BackgroundAction.cpp
    Thread.cpp
)

exile_lib(LibThreading threading)
target_link_libraries(LibThreading PRIVATE LibCore)
