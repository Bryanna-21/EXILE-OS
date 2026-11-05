exile_component(
    Tubes
    TARGETS Tubes
)

set(SOURCES
        Shapes.cpp
        Tubes.cpp
        main.cpp
)

exile_app(Tubes ICON app-tubes)
target_link_libraries(Tubes PRIVATE LibCore LibDesktop LibGfx LibGL LibGUI LibMain)
