set(SOURCES
    Card.cpp
    CardGame.cpp
    CardPainter.cpp
    CardStack.cpp
)

exile_lib(LibCards cards)
target_link_libraries(LibCards PRIVATE LibCore LibConfig LibGfx LibGUI)
