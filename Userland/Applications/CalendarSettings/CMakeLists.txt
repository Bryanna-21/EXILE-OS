exile_component(
    CalendarSettings
    RECOMMENDED
    TARGETS CalendarSettings
)

compile_gml(CalendarSettingsWidget.gml CalendarSettingsWidgetGML.cpp)

set(SOURCES
    main.cpp
    CalendarSettingsWidgetGML.cpp
    CalendarSettingsWidget.cpp
)

exile_app(CalendarSettings ICON app-calendar)
target_link_libraries(CalendarSettings PRIVATE LibConfig LibCore LibGfx LibGUI LibMain)
