/*
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Verify that a higher pthread scheduling priority receives CPU time ahead
// of a lower-priority CPU-bound thread.

#include <AK/Atomic.h>
#include <AK/Format.h>
#include <errno.h>
#include <LibTest/TestCase.h>
#include <pthread.h>
#include <sched.h>
#include <time.h>
#include <unistd.h>

namespace {

constexpr int high_priority = 50;
constexpr int normal_priority = 30;
constexpr int controller_priority = 99;

struct Worker {
    pthread_t thread {};
    Atomic<u64> count { 0 };
};

pthread_mutex_t s_gate_mutex = PTHREAD_MUTEX_INITIALIZER;
pthread_cond_t s_gate_condition = PTHREAD_COND_INITIALIZER;
bool s_start = false;
Atomic<bool> s_stop { false };

void wait_for_start()
{
    pthread_mutex_lock(&s_gate_mutex);
    while (!s_start)
        pthread_cond_wait(&s_gate_condition, &s_gate_mutex);
    pthread_mutex_unlock(&s_gate_mutex);
}

void release_start_gate()
{
    pthread_mutex_lock(&s_gate_mutex);
    s_start = true;
    pthread_cond_broadcast(&s_gate_condition);
    pthread_mutex_unlock(&s_gate_mutex);
}

int create_thread_with_priority(pthread_t* thread, int priority, void* (*entry)(void*), void* argument)
{
    pthread_attr_t attributes;
    int rc = pthread_attr_init(&attributes);
    if (rc != 0)
        return rc;

    sched_param parameter { .sched_priority = priority };
    rc = pthread_attr_setschedparam(&attributes, &parameter);
    if (rc == 0)
        rc = pthread_create(thread, &attributes, entry, argument);

    pthread_attr_destroy(&attributes);
    return rc;
}

void* spin_worker(void* argument)
{
    auto& worker = *static_cast<Worker*>(argument);
    wait_for_start();

    // No syscalls, locks or yields in the measured loop.
    while (!s_stop.load(AK::MemoryOrder::memory_order_relaxed))
        worker.count.fetch_add(1, AK::MemoryOrder::memory_order_relaxed);

    return nullptr;
}

void* stop_controller(void*)
{
    wait_for_start();

    // This highest-priority thread sleeps, allowing the workers to compete.
    // Its timer wakeup guarantees that the test can stop even if the high
    // priority worker would otherwise starve the test's main thread.
    timespec remaining { .tv_sec = 0, .tv_nsec = 500 * 1000 * 1000 };
    while (nanosleep(&remaining, &remaining) < 0) {
        if (errno != EINTR) {
            s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
            return nullptr;
        }
    }

    s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
    return nullptr;
}

void stop_and_join_started_threads(
    bool high_started, pthread_t high_thread,
    bool normal_started, pthread_t normal_thread,
    bool controller_started, pthread_t controller_thread)
{
    s_stop.store(true, AK::MemoryOrder::memory_order_relaxed);
    release_start_gate();

    if (controller_started)
        pthread_join(controller_thread, nullptr);
    if (high_started)
        pthread_join(high_thread, nullptr);
    if (normal_started)
        pthread_join(normal_thread, nullptr);
}

}

TEST_CASE(higher_priority_thread_receives_more_cpu_time)
{
    long online_cpus = sysconf(_SC_NPROCESSORS_ONLN);
    if (online_cpus != 1) {
        dbgln("SCHED-PRIO-TEST: skipped; requires one online CPU, found {}", online_cpus);
        return;
    }

    Worker high_worker;
    Worker normal_worker;
    pthread_t controller_thread {};

    bool high_started = false;
    bool normal_started = false;
    bool controller_started = false;

    s_stop.store(false, AK::MemoryOrder::memory_order_relaxed);

    pthread_mutex_lock(&s_gate_mutex);
    s_start = false;
    pthread_mutex_unlock(&s_gate_mutex);

    int rc = create_thread_with_priority(&high_worker.thread, high_priority, spin_worker, &high_worker);
    if (rc == 0)
        high_started = true;

    if (rc == 0) {
        rc = create_thread_with_priority(&normal_worker.thread, normal_priority, spin_worker, &normal_worker);
        if (rc == 0)
            normal_started = true;
    }

    if (rc == 0) {
        rc = create_thread_with_priority(&controller_thread, controller_priority, stop_controller, nullptr);
        if (rc == 0)
            controller_started = true;
    }

    if (rc != 0) {
        stop_and_join_started_threads(
            high_started, high_worker.thread,
            normal_started, normal_worker.thread,
            controller_started, controller_thread);
        EXPECT_EQ(rc, 0);
        return;
    }

    release_start_gate();

    pthread_join(controller_thread, nullptr);
    pthread_join(high_worker.thread, nullptr);
    pthread_join(normal_worker.thread, nullptr);

    auto high_count = high_worker.count.load(AK::MemoryOrder::memory_order_relaxed);
    auto normal_count = normal_worker.count.load(AK::MemoryOrder::memory_order_relaxed);

    dbgln("SCHED-PRIO-TEST: cpus={} high_priority={} normal_priority={} high_count={} normal_count={}",
        online_cpus, high_priority, normal_priority, high_count, normal_count);

    EXPECT(high_count > 0);
    EXPECT(high_count >= normal_count * 10);

    dbgln("SCHED-PRIO-TEST: complete");
}
