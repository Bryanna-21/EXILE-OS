/*
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Checks that thread priority changes how CPU time is shared.
//
// One CPU-bound "high" spinner per online CPU keeps every CPU busy. A single CPU-bound "low" spinner at a lower
// priority competes with them. With strict priority between ready-queue buckets (Kernel/Tasks/SchedulerPriority.h) the
// low spinner gets almost no CPU time. Before the priority mapping was fixed all priorities above the minimum shared one
// bucket, and the low spinner got a fair share, which fails the check below.
//
// The spinners make no syscalls and never yield. The main thread runs at the maximum priority so it can always wake up
// to stop them, which means a kernel with broken priorities fails an assertion here instead of hanging.
//
// A pass on a host OS only shows that the test logic is sound. The evidence that matters is the serial log of a QEMU
// boot of this kernel (look for the SCHED-PRIO-TEST lines).

#include <AK/Atomic.h>
#include <AK/Format.h>
#include <AK/NonnullOwnPtr.h>
#include <AK/NumericLimits.h>
#include <AK/Vector.h>
#include <LibTest/TestCase.h>
#include <pthread.h>
#include <sched.h>
#include <time.h>
#include <unistd.h>

namespace {

struct Spinner {
    pthread_t thread {};
    int priority { 0 };
    Atomic<u64> count { 0 };
};

Atomic<bool> s_stop { false };

void* spin(void* argument)
{
    auto& spinner = *static_cast<Spinner*>(argument);
    struct sched_param parameters { .sched_priority = spinner.priority };
    pthread_setschedparam(pthread_self(), 0, &parameters);
    while (!s_stop.load(AK::MemoryOrder::memory_order_relaxed))
        spinner.count.fetch_add(1, AK::MemoryOrder::memory_order_relaxed);
    return nullptr;
}

}

TEST_CASE(higher_priority_threads_get_the_cpu)
{
    long online_cpus = sysconf(_SC_NPROCESSORS_ONLN);
    if (online_cpus < 1)
        online_cpus = 1;

    constexpr int high_priority = 50;
    constexpr int low_priority = 30;

    // The main thread must outrank the spinners so it can stop them.
    struct sched_param main_parameters { .sched_priority = sched_get_priority_max(0) };
    int const main_priority_result = pthread_setschedparam(pthread_self(), 0, &main_parameters);
    if (main_priority_result != 0) {
        EXPECT_EQ(main_priority_result, 0);
        return;
    }

    s_stop.store(false);
    Vector<NonnullOwnPtr<Spinner>> high_spinners;
    for (long i = 0; i < online_cpus; ++i) {
        auto spinner = make<Spinner>();
        spinner->priority = high_priority;
        int const rc = pthread_create(&spinner->thread, nullptr, spin, spinner.ptr());
        if (rc != 0) {
            s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
            for (auto& started : high_spinners)
                pthread_join(started->thread, nullptr);
            EXPECT_EQ(rc, 0);
            return;
        }
        high_spinners.append(move(spinner));
    }
    auto low_spinner = make<Spinner>();
    low_spinner->priority = low_priority;
    int const low_rc = pthread_create(&low_spinner->thread, nullptr, spin, low_spinner.ptr());
    if (low_rc != 0) {
        s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
        for (auto& started : high_spinners)
            pthread_join(started->thread, nullptr);
        EXPECT_EQ(low_rc, 0);
        return;
    }

    struct timespec const duration { .tv_sec = 1, .tv_nsec = 0 };
    nanosleep(&duration, nullptr);
    s_stop.store(true);

    u64 high_minimum = NumericLimits<u64>::max();
    for (auto& spinner : high_spinners) {
        pthread_join(spinner->thread, nullptr);
        high_minimum = min(high_minimum, spinner->count.load());
    }
    pthread_join(low_spinner->thread, nullptr);
    u64 const low_count = low_spinner->count.load();

    // The serial verifier and humans read this line.
    dbgln("SCHED-PRIO-TEST: cpus={} high_priority={} low_priority={} high_min_count={} low_count={}", online_cpus, high_priority, low_priority, high_minimum, low_count);

    // Every CPU was kept busy by a higher-priority thread, so the lower-priority thread may only run in small gaps.
    bool const high_ran = high_minimum > 0;
    bool const low_limited = low_count * 10 <= high_minimum;
    EXPECT(high_ran);
    EXPECT(low_limited);

    // The verdict line is printed whether the checks passed or not, so logs can be judged without trusting a "complete" marker.
    dbgln("SCHED-PRIO-TEST: result={}", high_ran && low_limited ? "pass" : "fail");
}
