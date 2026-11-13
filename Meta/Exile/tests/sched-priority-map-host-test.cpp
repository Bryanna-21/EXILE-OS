// Host test for Kernel/Tasks/SchedulerPriority.h. Compiling it already runs the header's static_asserts.
// Includes a negative control: the previous formula must be shown to collapse priorities, so the test can fail.
#include <Kernel/Tasks/SchedulerPriority.h>
#include <cstdio>

using namespace Kernel;

static u32 old_mapping(u32 priority)
{
    constexpr u32 count = THREAD_PRIORITY_MAX - THREAD_PRIORITY_MIN + 1;
    return ((count - (priority - THREAD_PRIORITY_MIN)) / count) * (ready_queue_bucket_count - 1);
}

static int distinct_buckets(u32 (*mapping)(u32))
{
    int seen[ready_queue_bucket_count] = {};
    int distinct = 0;
    for (u32 p = THREAD_PRIORITY_MIN; p <= THREAD_PRIORITY_MAX; ++p) {
        u32 bucket = mapping(p);
        if (bucket >= ready_queue_bucket_count)
            return -1;
        if (!seen[bucket]++)
            ++distinct;
    }
    return distinct;
}

int main()
{
    int failures = 0;
    auto check = [&](bool ok, char const* what) {
        std::printf("%s: %s\n", ok ? "PASS" : "FAIL", what);
        failures += !ok;
    };

    check(thread_priority_to_ready_queue_index(99) == 0, "priority 99 -> bucket 0");
    check(thread_priority_to_ready_queue_index(50) == 15, "priority 50 -> bucket 15");
    check(thread_priority_to_ready_queue_index(30) == 21, "priority 30 -> bucket 21");
    check(thread_priority_to_ready_queue_index(10) == 28, "priority 10 -> bucket 28");
    check(thread_priority_to_ready_queue_index(1) == 31, "priority 1 -> bucket 31");
    check(distinct_buckets(thread_priority_to_ready_queue_index) == 32, "all 32 buckets are used");

    bool monotonic = true;
    for (u32 p = THREAD_PRIORITY_MIN + 1; p <= THREAD_PRIORITY_MAX; ++p)
        monotonic &= thread_priority_to_ready_queue_index(p) <= thread_priority_to_ready_queue_index(p - 1);
    check(monotonic, "mapping is non-increasing in priority");

    // Negative control: the formula this replaces.
    check(distinct_buckets(old_mapping) == 2, "negative control: old formula collapses 99 priorities into 2 buckets");
    check(old_mapping(50) == old_mapping(30) && old_mapping(30) == old_mapping(10), "negative control: old formula gave 10/30/50 the same bucket");

    std::printf("%s\n", failures ? "RESULT FAIL" : "RESULT PASS");
    return failures ? 1 : 0;
}
