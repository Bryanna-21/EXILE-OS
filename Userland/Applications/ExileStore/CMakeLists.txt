# Native Exile OS application target.
# The standalone host build is preserved in CMakeLists.host.cmake.

exile_component(
    ExileStore
    RECOMMENDED
    TARGETS ExileStore
)

set(SOURCES
    src/CatalogLoader.cpp
    src/native_main.cpp
)

exile_app(ExileStore ICON exile-store)

target_link_libraries(ExileStore PRIVATE
    LibCore
    LibGfx
    LibGUI
    LibMain
)

# The app opens /usr/share/exile-store/catalog/catalog.json. CMAKE_INSTALL_PREFIX is <build>/Root, which Meta/build-root-filesystem.sh
# rsyncs onto the image root, so the relative destination below must be "usr/share/exile-store" (the previous
# "share/exile-store" ended up at /share/exile-store on the image). Same convention as exile_install_sources() (usr/src/...).
install(
    DIRECTORY data/
    DESTINATION usr/share/exile-store
)
