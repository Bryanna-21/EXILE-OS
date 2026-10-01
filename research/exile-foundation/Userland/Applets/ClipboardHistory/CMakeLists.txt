exile_component(
    ClipboardHistory.Applet
    REQUIRED
    TARGETS ClipboardHistory.Applet
)

set(SOURCES
    ClipboardHistoryModel.cpp
    main.cpp
)

exile_app(ClipboardHistory.Applet ICON edit-copy)
target_link_libraries(ClipboardHistory.Applet PRIVATE LibGUI LibCore LibGfx LibConfig LibMain)
