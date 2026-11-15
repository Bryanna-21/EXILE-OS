/*
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Checks that the kernel's finalizer keeps reclaiming exited processes while normal-priority threads keep every CPU busy.
//
// The finalizer is a kernel thread that finishes off dying threads and processes. A parent only sees a child as reaped
// (waitpid returns) after the finalizer has processed it. If the finalizer were scheduled below normal threads it would
// starve behind CPU-bound work: children would pile up and waitpid would not return. That is exactly the failure that
// Kernel/Tasks/FinalizerTask.cpp avoids by running at THREAD_PRIORITY_NORMAL.
//
// One CPU-bound spinner per online CPU runs at the default (normal) priority and makes no syscalls. The test then forks
// children that exit immediately and requires all of them to be reaped within a generous deadline.
//
// A pass on a host OS only shows that the test logic works. The evidence that matters is the serial/debug log of a QEMU
// boot of this kernel (look for the FINALIZER-TEST lines).

#include <AK/Atomic.h>
#include <AK/Format.h>
#include <AK/NonnullOwnPtr.h>
#include <AK/Vector.h>
#include <LibTest/TestCase.h>
#include <errno.h>
#include <pthread.h>
#include <sys/wait.h>
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
    while (!s_stop.load(AK::MemoryOrder::memory_order_relaxed))
        spinner.count.fetch_add(1, AK::MemoryOrder::memory_order_relaxed);
    return nullptr;
}

u64 monotonic_milliseconds()
{
    struct timespec now {};
    clock_gettime(CLOCK_MONOTONIC, &now);
    return static_cast<u64>(now.tv_sec) * 1000 + static_cast<u64>(now.tv_nsec) / 1000000;
}

}

TEST_CASE(finalizer_reaps_children_while_cpus_are_busy)
{
    long online_cpus = sysconf(_SC_NPROCESSORS_ONLN);
    if (online_cpus < 1)
        online_cpus = 1;
    constexpr int child_count = 20;
    constexpr u64 deadline_ms = 20 * 1000;

    s_stop.store(false);
    Vector<NonnullOwnPtr<Spinner>> spinners;
    for (long i = 0; i < online_cpus; ++i) {
        auto spinner = make<Spinner>();
        int const rc = pthread_create(&spinner->thread, nullptr, spin, spinner.ptr());
        if (rc != 0) {
            s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
            for (auto& started_spinner : spinners)
                pthread_join(started_spinner->thread, nullptr);
            dbgln("FINALIZER-TEST: result=fail");
            EXPECT_EQ(rc, 0);
            return;
        }
        spinners.append(move(spinner));
    }

    int started = 0;
    for (int i = 0; i < child_count; ++i) {
        pid_t pid = fork();
        if (pid == 0)
            _exit(0);
        if (pid > 0)
            ++started;
    }

    u64 const start = monotonic_milliseconds();
    int reaped = 0;
    int bad_exit_status = 0;
    bool wait_failed = false;
    while (reaped < started && monotonic_milliseconds() - start < deadline_ms) {
        int status = 0;
        pid_t pid = waitpid(-1, &status, WNOHANG);
        if (pid > 0) {
            ++reaped;
            if (!WIFEXITED(status) || WEXITSTATUS(status) != 0)
                ++bad_exit_status;
            continue;
        }
        if (pid < 0 && errno != EINTR) {
            wait_failed = true;
            break;
        }
        struct timespec const pause { .tv_sec = 0, .tv_nsec = 10 * 1000 * 1000 };
        nanosleep(&pause, nullptr);
    }
    u64 const elapsed = monotonic_milliseconds() - start;

    s_stop.store(true);
    for (auto& spinner : spinners)
        pthread_join(spinner->thread, nullptr);

    // The serial verifier and humans read this line.
    dbgln("FINALIZER-TEST: cpus={} started={} reaped={} elapsed_ms={}", online_cpus, started, reaped, elapsed);

    EXPECT_EQ(started, child_count);
    EXPECT_EQ(reaped, started);
    EXPECT_EQ(bad_exit_status, 0);
    EXPECT(!wait_failed);

    // Printed whether the checks passed or not, so logs can be judged without trusting a "complete" marker.
    bool const passed = started == child_count && reaped == started
        && bad_exit_status == 0 && !wait_failed;
    dbgln("FINALIZER-TEST: result={}", passed ? "pass" : "fail");
}
