set(KERNEL_WAIT_SOURCES
    TestWait.cpp
    TestWaitDoesReap.cpp
    TestWaitFirstNoHang.cpp
    TestWaitNoChild.cpp
)

foreach(wait_source IN LISTS KERNEL_WAIT_SOURCES)
    exile_test("${wait_source}" Kernel)
endforeach()
