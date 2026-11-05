exile_component(
    Audio.Applet
    REQUIRED
    TARGETS Audio.Applet
)

set(SOURCES
    main.cpp
)

exile_app(Audio.Applet ICON audio-volume-high)
target_link_libraries(Audio.Applet PRIVATE LibGUI LibGfx LibAudio LibCore LibConfig LibIPC LibMain)
