exile_component(
    KeyboardPreferenceLoader
    REQUIRED
    TARGETS KeyboardPreferenceLoader keymap
)

set(SOURCES
    main.cpp
)

exile_bin(KeyboardPreferenceLoader)
target_link_libraries(KeyboardPreferenceLoader PRIVATE LibCore LibMain)
