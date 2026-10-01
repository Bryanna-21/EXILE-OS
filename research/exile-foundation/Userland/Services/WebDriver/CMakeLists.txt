exile_component(
    WebDriver
    TARGETS WebDriver
    DEPENDS WebContent
)

set(SOURCES
    Client.cpp
    Session.cpp
    WebContentConnection.cpp
    main.cpp
)

set(GENERATED_SOURCES
    ../../Services/WebContent/WebDriverClientEndpoint.h
    ../../Services/WebContent/WebDriverServerEndpoint.h
)

exile_bin(WebDriver)
target_link_libraries(WebDriver PRIVATE LibCore LibHTTP LibMain LibIPC LibWeb LibGfx LibWebView LibURL)
