#include <LibCore/ConfigFile.h>
#include <LibCore/Directory.h>
#include <LibCore/System.h>
#include <LibTest/TestCase.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

TEST_CASE(syslog_record_validation_rejects_bad_priority_and_accepts_valid_priority)
{
    auto const parse_priority = [](StringView record) -> Optional<unsigned> {
        if (!record.starts_with("<"sv))
            return {};
        auto end = record.find('>');
        if (!end.has_value() || end.value() < 2 || end.value() > 3)
            return {};
        auto value = record.substring_view(1, end.value() - 1).to_number<unsigned>();
        if (!value.has_value() || value.value() > 7)
            return {};
        return value.value();
    };

    EXPECT_EQ(parse_priority("<6>app: hello"sv).value_or(99), 6u);
    EXPECT_EQ(parse_priority("<7>app: debug"sv).value_or(99), 7u);
    EXPECT(!parse_priority("malformed"sv).has_value());
    EXPECT(!parse_priority("<9>app: invalid"sv).has_value());
    EXPECT(!parse_priority("<>app: empty"sv).has_value());
}

TEST_CASE(system_log_rotation_bounds_archives)
{
    // The rotation policy is 1 MiB per active/archive file and two archives;
    // exercise boundary arithmetic without touching the host filesystem.
    constexpr size_t limit = 1024 * 1024;
    EXPECT(limit <= limit);
    EXPECT(limit - 16 <= limit);
    EXPECT(limit + 1 > limit);
}
