exile_component(
    Gradient
    TARGETS Gradient
)

set(SOURCES
    Gradient.cpp
)

exile_app(Gradient ICON app-gradient)
target_link_libraries(Gradient PRIVATE LibDesktop LibGUI LibCore LibGfx LibMain)
