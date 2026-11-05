exile_component(
    Bridgeline
    RECOMMENDED
    DESCRIPTION "Bridgeline: the Exile OS web browser (LibWeb engine)"
    TARGETS Bridgeline
    DEPENDS BrowserSettings ImageDecoder RequestServer WebContent
)

stringify_gml(BrowserWindow.gml BridgelineBrowserWindowGML.h browser_window_gml)
stringify_gml(EditBookmark.gml BridgelineEditBookmarkGML.h edit_bookmark_gml)
stringify_gml(FindInPageWidget.gml BridgelineFindInPageWidgetGML.h find_in_page_widget_gml)
stringify_gml(History/HistoryWidget.gml BridgelineHistoryWidgetGML.h history_widget_gml)
stringify_gml(StorageWidget.gml BridgelineStorageWidgetGML.h storage_widget_gml)
stringify_gml(Tab.gml BridgelineTabGML.h tab_gml)

set(SOURCES
    BookmarksBarWidget.cpp
    BrowserWindow.cpp
    CookiesModel.cpp
    DownloadWidget.cpp
    FindInPageWidget.cpp
    History/HistoryModel.cpp
    History/HistoryWidget.cpp
    IconBag.cpp
    InspectorWidget.cpp
    StorageModel.cpp
    StorageWidget.cpp
    Tab.cpp
    TaskManagerWidget.cpp
    URLBox.cpp
    WindowActions.cpp
    main.cpp
)

set(GENERATED_SOURCES
    BridgelineBrowserWindowGML.h
    BridgelineEditBookmarkGML.h
    BridgelineFindInPageWidgetGML.h
    BridgelineHistoryWidgetGML.h
    BridgelineStorageWidgetGML.h
    BridgelineTabGML.h
)

exile_app(Bridgeline ICON app-browser)
target_link_libraries(Bridgeline PRIVATE LibCore LibWebView LibWeb LibProtocol LibGUI LibDesktop LibConfig LibGfx LibIPC LibJS LibLocale LibMain LibSyntax LibURL)
