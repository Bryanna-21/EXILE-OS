exile_component(
    XFiles
    RECOMMENDED
    DESCRIPTION "XFiles: file manager with a real Trash"
    TARGETS XFiles
    DEPENDS FileOperation
)

stringify_gml(XFiles_FileManagerWindow.gml XFiles_FileManagerWindowGML.h file_manager_window_gml)
stringify_gml(XFiles_FileOperationProgress.gml XFiles_FileOperationProgressGML.h file_operation_progress_gml)
stringify_gml(XFiles_PropertiesWindowArchiveTab.gml XFiles_PropertiesWindowArchiveTabGML.h properties_window_archive_tab_gml)
stringify_gml(XFiles_PropertiesWindowAudioTab.gml XFiles_PropertiesWindowAudioTabGML.h properties_window_audio_tab_gml)
stringify_gml(XFiles_PropertiesWindowFontTab.gml XFiles_PropertiesWindowFontTabGML.h properties_window_font_tab_gml)
stringify_gml(XFiles_PropertiesWindowGeneralTab.gml XFiles_PropertiesWindowGeneralTabGML.h properties_window_general_tab_gml)
stringify_gml(XFiles_PropertiesWindowImageTab.gml XFiles_PropertiesWindowImageTabGML.h properties_window_image_tab_gml)
stringify_gml(XFiles_PropertiesWindowPDFTab.gml XFiles_PropertiesWindowPDFTabGML.h properties_window_pdf_tab_gml)

set(SOURCES
    DesktopWidget.cpp
    DirectoryView.cpp
    FileOperationProgressWidget.cpp
    FileUtils.cpp
    main.cpp
    PropertiesWindow.cpp
    TrashActions.cpp
)

set(GENERATED_SOURCES
    XFiles_FileManagerWindowGML.h
    XFiles_FileOperationProgressGML.h
    XFiles_PropertiesWindowArchiveTabGML.h
    XFiles_PropertiesWindowAudioTabGML.h
    XFiles_PropertiesWindowFontTabGML.h
    XFiles_PropertiesWindowGeneralTabGML.h
    XFiles_PropertiesWindowImageTabGML.h
    XFiles_PropertiesWindowPDFTabGML.h
)

exile_app(XFiles ICON app-file-manager)
target_link_libraries(XFiles PRIVATE LibArchive LibAudio LibConfig LibCore LibDesktop LibFileSystem LibGfx LibGUI LibMain LibMaps LibPDF LibThreading LibURL)
