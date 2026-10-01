exile_component(
    Mandelbrot
    TARGETS Mandelbrot
)

set(SOURCES
    Mandelbrot.cpp
)

exile_app(Mandelbrot ICON app-mandelbrot)
target_link_libraries(Mandelbrot PRIVATE LibCore LibFileSystemAccessClient LibGfx LibGUI LibMain)
