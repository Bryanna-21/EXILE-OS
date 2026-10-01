exile_component(
    CrashReporter
    REQUIRED
    TARGETS CrashReporter
)

compile_gml(CrashReporterWindow.gml CrashReporterWindowGML.cpp)


set(SOURCES
    CrashReporterWindowGML.cpp
    main.cpp
)

exile_app(CrashReporter ICON app-crash-reporter)
target_link_libraries(CrashReporter PRIVATE LibCore LibCoredump LibDebug LibDesktop LibELF LibFileSystem LibFileSystemAccessClient LibGfx LibGUI LibMain LibThreading LibURL)
