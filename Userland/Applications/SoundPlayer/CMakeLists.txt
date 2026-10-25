exile_component(
    Audio
    RECOMMENDED
    TARGETS Audio
    DEPENDS AudioServer
)

set(SOURCES
    AlbumCoverVisualizationWidget.cpp
    BarsVisualizationWidget.cpp
    M3UParser.cpp
    PlaybackManager.cpp
    Player.cpp
    Playlist.cpp
    PlaylistWidget.cpp
    SampleWidget.cpp
    SoundPlayerWidget.cpp
    main.cpp
)

exile_app(Audio ICON app-sound-player)
target_link_libraries(Audio PRIVATE LibAudio LibConfig LibCore LibFileSystem LibDSP LibGfx LibGUI LibIPC LibMain LibThreading LibImageDecoderClient LibURL)
