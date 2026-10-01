exile_component(
    Settings
    REQUIRED
    TARGETS Settings
)

set(SOURCES
    main.cpp
)

exile_app(Settings ICON app-settings)
target_link_libraries(Settings PRIVATE LibCore LibGfx LibGUI LibDesktop LibMain)
