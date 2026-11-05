set(SOURCES
    Editor.cpp
    InternalFunctions.cpp
    KeyCallbackMachine.cpp
    SuggestionManager.cpp
    XtermSuggestionDisplay.cpp
)

exile_lib(LibLine line)
target_link_libraries(LibLine PRIVATE LibCoreMinimal LibCoreBasic LibUnicode LibFileSystem)
