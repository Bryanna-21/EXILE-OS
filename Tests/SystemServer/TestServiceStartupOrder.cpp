#include <Userland/Services/SystemServer/ServiceStartupOrder.h>
#include <LibTest/TestCase.h>

using namespace SystemServer;

static HashMap<ByteString, Vector<ByteString>> dependencies_for(std::initializer_list<std::pair<ByteString, Vector<ByteString>>> entries)
{
    HashMap<ByteString, Vector<ByteString>> dependencies;
    for (auto const& entry : entries)
        dependencies.set(entry.first, entry.second);
    return dependencies;
}

static bool contains(Vector<size_t> const& values, size_t value)
{
    return values.contains_slow(value);
}

TEST_CASE(dependency_order_is_stable_and_dependency_first)
{
    Vector<ByteString> services { "Consumer"sv, "Independent"sv, "Provider"sv };
    auto dependencies = dependencies_for({ { "Consumer"sv, { "Provider"sv } } });
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (order.is_error())
        return;
    EXPECT_EQ(order.value().size(), 3u);
    if (order.value().size() == 3u) {
        EXPECT_EQ(order.value()[0], 1u);
        EXPECT_EQ(order.value()[1], 2u);
        EXPECT_EQ(order.value()[2], 0u);
    }
}

TEST_CASE(missing_dependency_blocks_service_and_dependents)
{
    Vector<ByteString> services { "Consumer"sv, "Middle"sv, "Independent"sv };
    auto dependencies = dependencies_for({
        { "Consumer"sv, { "Middle"sv } },
        { "Middle"sv, { "Missing"sv } },
    });
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (order.is_error())
        return;
    EXPECT_EQ(order.value().size(), 1u);
    if (order.value().size() == 1u)
        EXPECT_EQ(order.value()[0], 2u);
}

TEST_CASE(circular_dependency_is_rejected)
{
    Vector<ByteString> services { "First"sv, "Second"sv };
    auto dependencies = dependencies_for({
        { "First"sv, { "Second"sv } },
        { "Second"sv, { "First"sv } },
    });
    EXPECT(resolve_service_startup_order(services, dependencies).is_error());
}

TEST_CASE(self_dependency_is_rejected)
{
    Vector<ByteString> services { "Self"sv };
    auto dependencies = dependencies_for({ { "Self"sv, { "Self"sv } } });
    EXPECT(resolve_service_startup_order(services, dependencies).is_error());
}

TEST_CASE(duplicate_service_names_are_rejected)
{
    Vector<ByteString> services { "Duplicate"sv, "Duplicate"sv };
    HashMap<ByteString, Vector<ByteString>> dependencies;
    EXPECT(resolve_service_startup_order(services, dependencies).is_error());
}

TEST_CASE(empty_service_graph_is_valid)
{
    Vector<ByteString> services;
    HashMap<ByteString, Vector<ByteString>> dependencies;
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (!order.is_error())
        EXPECT(order.value().is_empty());
}

TEST_CASE(duplicate_dependencies_do_not_duplicate_services)
{
    Vector<ByteString> services { "Dependent"sv, "Provider"sv };
    auto dependencies = dependencies_for({ { "Dependent"sv, { "Provider"sv, "Provider"sv } } });
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (order.is_error())
        return;
    EXPECT_EQ(order.value().size(), 2u);
    if (order.value().size() == 2u) {
        EXPECT_EQ(order.value()[0], 1u);
        EXPECT_EQ(order.value()[1], 0u);
    }
}

TEST_CASE(long_dependency_chain_is_ordered)
{
    Vector<ByteString> services { "A"sv, "B"sv, "C"sv, "D"sv };
    auto dependencies = dependencies_for({
        { "A"sv, { "B"sv } },
        { "B"sv, { "C"sv } },
        { "C"sv, { "D"sv } },
    });
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (order.is_error())
        return;
    EXPECT_EQ(order.value().size(), 4u);
    if (order.value().size() == 4u) {
        EXPECT_EQ(order.value()[0], 3u);
        EXPECT_EQ(order.value()[1], 2u);
        EXPECT_EQ(order.value()[2], 1u);
        EXPECT_EQ(order.value()[3], 0u);
    }
}

TEST_CASE(multiple_dependents_follow_one_provider)
{
    Vector<ByteString> services { "ClientA"sv, "ClientB"sv, "Provider"sv };
    auto dependencies = dependencies_for({
        { "ClientA"sv, { "Provider"sv } },
        { "ClientB"sv, { "Provider"sv } },
    });
    auto order = resolve_service_startup_order(services, dependencies);
    EXPECT(!order.is_error());
    if (order.is_error())
        return;
    EXPECT_EQ(order.value().size(), 3u);
    if (order.value().size() == 3u) {
        auto provider_position = order.value().find(2).value_or(3);
        auto client_a_position = order.value().find(0).value_or(3);
        auto client_b_position = order.value().find(1).value_or(3);
        EXPECT(provider_position < client_a_position);
        EXPECT(provider_position < client_b_position);
    }
}
