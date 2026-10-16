# Native Exile OS application target.
# The standalone host build is preserved in CMakeLists.host.cmake.

exile_component(
    ExileStore
    RECOMMENDED
    TARGETS ExileStore
)

set(SOURCES
    src/native_main.cpp
)

exile_app(ExileStore ICON app-help)

target_link_libraries(ExileStore PRIVATE
    LibCore
    LibGfx
    LibGUI
    LibMain
)

install(
    DIRECTORY data/
    DESTINATION share/exile-store
)
