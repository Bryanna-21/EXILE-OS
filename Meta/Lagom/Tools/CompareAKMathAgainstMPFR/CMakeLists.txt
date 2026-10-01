find_library(MPFR_LIBRARY
    NAMES mpfr
)

find_path(MPFR_INCLUDE_DIR
    NAMES mpfr.h
)

if (NOT MPFR_LIBRARY OR NOT MPFR_INCLUDE_DIR)
    message(STATUS "MPFR not found; disabling CompareAKMathAgainstMPFR")
    return()
endif()

set(SOURCES
    main.cpp
    reference_functions.cpp
)

lagom_tool(CompareAKMathAgainstMPFR
    LIBS LibMain LibCoreMinimal LibFileSystem ${MPFR_LIBRARY}
)
target_include_directories(CompareAKMathAgainstMPFR PRIVATE ${MPFR_INCLUDE_DIR})
