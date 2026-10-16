/*
 * Exile Store native catalog browser.
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/JsonArray.h>
#include <AK/JsonObject.h>
#include <AK/JsonValue.h>
#include <AK/StringView.h>
#include <AK/Try.h>
#include <LibCore/File.h>
#include <LibCore/System.h>
#include <LibGUI/Application.h>
#include <LibGUI/BoxLayout.h>
#include <LibGUI/Icon.h>
#include <LibGUI/Label.h>
#include <LibGUI/ListView.h>
#include <LibGUI/TextBox.h>
#include <LibGUI/Window.h>
#include <LibGUI/ItemListModel.h>
#include <LibMain/Main.h>

using namespace AK::Literals;

struct StoreEntry {
    ByteString id;
    ByteString name;
    ByteString summary;
    ByteString description;
    ByteString publisher;
    ByteString version;
    ByteString category;
};

static ByteString field_or(JsonObject const& object, StringView key, StringView fallback)
{
    auto value = object.get_byte_string(key);
    if (!value.has_value())
        return ByteString(fallback);
    return value.release_value();
}

static Vector<StoreEntry> load_catalog(GUI::Label& status)
{
    Vector<StoreEntry> entries;

    auto file_or_error = Core::File::open(
        "/usr/share/exile-store/catalog/catalog.json"sv,
        Core::File::OpenMode::Read);

    if (file_or_error.is_error()) {
        status.set_text(
            "The Exile Store catalog could not be opened.\n"
            "Check that the catalog data is included in the OS build.");
        return entries;
    }

    auto file = file_or_error.release_value();
    auto bytes_or_error = file->read_until_eof();

    if (bytes_or_error.is_error()) {
        status.set_text("The Exile Store catalog could not be read.");
        return entries;
    }

    auto bytes = bytes_or_error.release_value();
    auto json_or_error = JsonValue::from_string(StringView { bytes });

    if (json_or_error.is_error()) {
        status.set_text("The Exile Store catalog contains invalid JSON.");
        return entries;
    }

    auto json = json_or_error.release_value();

    if (!json.is_object()) {
        status.set_text("The Exile Store catalog has an invalid structure.");
        return entries;
    }

    auto apps = json.as_object().get_array("apps"sv);

    if (!apps.has_value()) {
        status.set_text("The Exile Store catalog has no application list.");
        return entries;
    }

    for (auto const& item : apps.value()) {
        if (!item.is_object())
            continue;

        auto const& object = item.as_object();

        StoreEntry entry;
        entry.id = field_or(object, "appId"sv, "unknown"sv);
        entry.name = field_or(object, "name"sv, "Unnamed application"sv);
        entry.summary = field_or(object, "summary"sv, ""sv);
        entry.description = field_or(object, "description"sv, ""sv);
        entry.version = field_or(object, "currentVersion"sv, "Unknown"sv);

        if (auto publisher = object.get_object("publisher"sv); publisher.has_value())
            entry.publisher = field_or(publisher.value(), "name"sv, "Unknown publisher"sv);
        else
            entry.publisher = "Unknown publisher";

        if (auto categories = object.get_array("categories"sv);
            categories.has_value() && !categories->is_empty()) {
            auto const& first_category = categories->at(0);
            if (first_category.is_string())
                entry.category = first_category.as_string();
        }

        entries.append(move(entry));
    }

    status.set_text(ByteString::formatted(
        "{} catalog entries loaded. Development catalog only; installation and launching are not enabled.",
        entries.size()));

    return entries;
}

ErrorOr<int> serenity_main(Main::Arguments arguments)
{
    TRY(Core::System::pledge("stdio recvfd sendfd rpath"));
    TRY(Core::System::unveil("/usr/share/exile-store", "r"));
    TRY(Core::System::unveil("/res", "r"));
    TRY(Core::System::unveil(nullptr, nullptr));

    auto app = TRY(GUI::Application::create(arguments));

    auto icon = GUI::Icon::default_icon("app-help"sv);
    auto window = GUI::Window::construct();
    window->set_title("Exile Store");
    window->set_icon(icon.bitmap_for_size(16));
    window->resize(760, 560);

    auto root = window->set_main_widget<GUI::Widget>();
    root->set_layout<GUI::VerticalBoxLayout>(GUI::Margins { 10, 10, 10, 10 }, 6);

    auto& heading = root->add<GUI::Label>("Exile Store");
    auto& subtitle = root->add<GUI::Label>(
        "Discover applications for Exile OS. This build provides catalog browsing only.");

    auto& search = root->add<GUI::TextBox>();
    search.set_placeholder("Search applications...");

    auto& list = root->add<GUI::ListView>();
    auto& details = root->add<GUI::Label>(
        "Select an application to view its details.");

    auto& status = root->add<GUI::Label>("Loading catalog...");

    auto entries = load_catalog(status);
    Vector<size_t> visible_entries;
    RefPtr<GUI::Model> list_model;

    auto refresh_list = [&](StringView query) {
        Vector<ByteString> names;
        visible_entries.clear();

        for (size_t i = 0; i < entries.size(); ++i) {
            auto const& entry = entries[i];

            auto matches = query.is_empty()
                || entry.name.contains(query, CaseSensitivity::CaseInsensitive)
                || entry.summary.contains(query, CaseSensitivity::CaseInsensitive)
                || entry.category.contains(query, CaseSensitivity::CaseInsensitive);

            if (!matches)
                continue;

            names.append(entry.name);
            visible_entries.append(i);
        }

        list_model = GUI::ItemListModel<ByteString>::create(names);
        list.set_model(*list_model);

        if (visible_entries.is_empty()) {
            details.set_text("No matching applications.");
        } else {
            details.set_text(
                "Select an application from the list to see its details.");
        }

        status.set_text(ByteString::formatted(
            "{} of {} catalog entries shown. Install and launch are not available in this build.",
            visible_entries.size(), entries.size()));
    };

    search.on_change = [&] {
        refresh_list(search.text());
    };

    list.on_selection_change = [&] {
        auto index = list.selection().first();
        auto row = index.row();

        if (row < 0 || static_cast<size_t>(row) >= visible_entries.size())
            return;

        auto const& entry = entries[visible_entries[static_cast<size_t>(row)]];

        details.set_text(MUST(String::formatted(
            "{}\n\n{}\n\n{}\n\nPublisher: {}\nVersion: {}\nCategory: {}\nApplication ID: {}\n\nStatus: Catalog entry only. Installation and launching are not implemented.",
            entry.name,
            entry.summary,
            entry.description,
            entry.publisher,
            entry.version,
            entry.category.is_empty() ? "Uncategorized" : entry.category,
            entry.id)));
    };

    refresh_list(""sv);

    window->show();
    return app->exec();
}
