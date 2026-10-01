include(${ExileOS_SOURCE_DIR}/Meta/CMake/pnp_ids.cmake)

set(SOURCES
    DMT.cpp
    EDID.cpp
    VIC.cpp
    ${PNP_IDS_SOURCES}
)

exile_lib(LibEDID edid)
target_compile_definitions(LibEDID PRIVATE ENABLE_PNP_IDS_DATA=$<BOOL:${ENABLE_PNP_IDS_DOWNLOAD}>)
