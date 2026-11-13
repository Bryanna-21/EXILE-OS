/*
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Runtime check for timer-driven preemption of CPU-bound threads.
//
// The kernel preempts through Scheduler::timer_tick() -> Processor::invoke_scheduler_async() -> a switch at trap exit
// (see Documentation/Kernel/PREEMPTION.md). This test needs that path to finish:
//
//  * It starts more purely CPU-bound threads than there are online CPUs. The loops make no syscalls and never yield, so
//    a thread can only get CPU time if the kernel takes a processor away from another spinning thread.
//  * The main thread sleeps. Waking it also needs a CPU, so a kernel that never preempts spinners hangs here and the
//    test runner's timeout reports the failure. A hang is the expected failure mode, not a bug in the test.
//
// Passing on a host OS only shows that the test logic is sound. It says nothing about this kernel. The result that
// matters is the serial log of a QEMU boot, checked by Meta/Exile/verify-preemption-serial.py.

#include <AK/Atomic.h>
#include <AK/Format.h>
#include <AK/NonnullOwnPtr.h>
#include <AK/NumericLimits.h>
#include <AK/Vector.h>
#include <LibTest/TestCase.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>

namespace {

struct Spinner {
    pthread_t thread {};
    Atomic<u64> count { 0 };
};

Atomic<bool> s_stop { false };

void* spin(void* argument)
{
    auto& spinner = *static_cast<Spinner*>(argument);
    // Deliberately no syscalls, locks or yields in this loop: only the timer interrupt can end this thread's slice.
    while (!s_stop.load(AK::MemoryOrder::memory_order_relaxed))
        spinner.count.fetch_add(1, AK::MemoryOrder::memory_order_relaxed);
    return nullptr;
}

}

TEST_CASE(cpu_bound_threads_are_preempted)
{
    long online_cpus = sysconf(_SC_NPROCESSORS_ONLN);
    if (online_cpus < 1)
        online_cpus = 1;
    size_t const thread_count = static_cast<size_t>(online_cpus) + 1;

    s_stop.store(false);
    Vector<NonnullOwnPtr<Spinner>> spinners;
    for (size_t i = 0; i < thread_count; ++i) {
        auto spinner = make<Spinner>();
        int rc = pthread_create(&spinner->thread, nullptr, spin, spinner.ptr());
        if (rc != 0) {
            // Stop and join every thread already created before reporting the failure.
            s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
            for (auto& started : spinners)
                pthread_join(started->thread, nullptr);
            EXPECT_EQ(rc, 0);
            return;
        }
        spinners.append(move(spinner));
    }

    // Needs a CPU to wake up, which needs preemption while every CPU is occupied by a spinner.
    struct timespec const duration { .tv_sec = 0, .tv_nsec = 500 * 1000 * 1000 };
    nanosleep(&duration, nullptr);
    s_stop.store(true);

    u64 minimum = NumericLimits<u64>::max();
    u64 maximum = 0;
    for (auto& spinner : spinners) {
        pthread_join(spinner->thread, nullptr);
        auto count = spinner->count.load();
        if (count < minimum)
            minimum = count;
        if (count > maximum)
            maximum = count;
    }

    // The serial verifier keys on this exact line.
    dbgln("PREEMPT-TEST: cpus={} threads={} min_count={} max_count={}", online_cpus, thread_count, minimum, maximum);

    // Every spinner ran, so none was starved by the others.
    EXPECT(minimum > 0);
    // Loose fairness bound: with equal-priority threads and a fixed slice, nobody should run 100x less than the busiest.
    // The bound is deliberately loose because emulation adds a lot of timing noise.
    EXPECT(minimum * 100 >= maximum);

    // Printed last, only when this point was reached: the serial verifier requires it.
    dbgln("PREEMPT-TEST: complete");
}
