/*
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <AK/Types.h>
// The THREAD_PRIORITY_* values come from the kernel's POSIX header. A host test can't include that header next to the
// host libc, so it passes the same values (read from the header by Meta/Exile/tests/test-sched-priority-map.sh) as macros.
#ifndef THREAD_PRIORITY_MAX
#    include <Kernel/API/POSIX/sched.h>
#endif

namespace Kernel {

// Number of ready-queue buckets. Must equal Scheduler.cpp's ThreadReadyQueues::count (asserted there).
inline constexpr u32 ready_queue_bucket_count = 32;

// Converts a thread priority in THREAD_PRIORITY_MIN..THREAD_PRIORITY_MAX to an index into g_ready_queues, where 0 is
// the highest-priority bucket. The mapping is linear and non-increasing in the priority.
//
// NOTE: The multiplication has to happen before the division. Dividing first is integer division, which is 0 for every
//       priority except THREAD_PRIORITY_MIN and put priorities 2..99 into a single bucket.
constexpr u32 thread_priority_to_ready_queue_index(u32 thread_priority)
{
    constexpr u32 priority_range = THREAD_PRIORITY_MAX - THREAD_PRIORITY_MIN;
    static_assert(priority_range > 0);
    return ((THREAD_PRIORITY_MAX - thread_priority) * (ready_queue_bucket_count - 1)) / priority_range;
}

namespace Detail {

constexpr bool ready_queue_mapping_is_valid()
{
    for (u32 priority = THREAD_PRIORITY_MIN; priority <= THREAD_PRIORITY_MAX; ++priority) {
        auto index = thread_priority_to_ready_queue_index(priority);
        if (index >= ready_queue_bucket_count)
            return false;
        if (priority > THREAD_PRIORITY_MIN && index > thread_priority_to_ready_queue_index(priority - 1))
            return false; // A higher priority must never land in a lower-priority bucket.
    }
    return true;
}

}

static_assert(thread_priority_to_ready_queue_index(THREAD_PRIORITY_MAX) == 0);
static_assert(thread_priority_to_ready_queue_index(THREAD_PRIORITY_MIN) == ready_queue_bucket_count - 1);
static_assert(Detail::ready_queue_mapping_is_valid());
// The priorities in use must not share a bucket (this is what the old formula got wrong).
static_assert(thread_priority_to_ready_queue_index(THREAD_PRIORITY_HIGH) < thread_priority_to_ready_queue_index(THREAD_PRIORITY_NORMAL));
static_assert(thread_priority_to_ready_queue_index(THREAD_PRIORITY_NORMAL) < thread_priority_to_ready_queue_index(THREAD_PRIORITY_LOW));
static_assert(thread_priority_to_ready_queue_index(THREAD_PRIORITY_LOW) < thread_priority_to_ready_queue_index(THREAD_PRIORITY_MIN));

}
