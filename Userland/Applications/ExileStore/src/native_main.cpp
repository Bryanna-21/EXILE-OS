/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Exile Store native catalog browser.
//
// What this build does: loads the development catalog from /usr/share/exile-store, lets the user search and select
// entries, and shows what the catalog says about them (publisher, version, permissions, package builds).
//
// What it deliberately does NOT do: install, update, uninstall or launch anything, and it does not verify catalog or
// package signatures. Exile OS has no privileged package-installer service yet, so these actions are shown as
// unavailable instead of being faked. See docs/INTEGRATION.md.

#include "CatalogLoader.h"
#include <AK/StringBuilder.h>
#include <AK/Try.h>
#include <LibCore/File.h>
#include <LibCore/System.h>
#include <LibGUI/Application.h>
#include <LibGUI/BoxLayout.h>
#include <LibGUI/Button.h>
#include <LibGUI/Icon.h>
#include <LibGUI/Label.h>
#include <LibGUI/ListView.h>
#include <LibGUI/Model.h>
#include <LibGUI/TextBox.h>
#include <LibGUI/Window.h>
#include <LibMain/Main.h>

static constexpr StringView catalog_path = "/usr/share/exile-store/catalog/catalog.json"sv;
static constexpr char catalog_directory[] = "/usr/share/exile-store";

static String to_label_text(ByteString const& text)
{
    return String::from_utf8_with_replacement_character(text);
}

// The list model owns the strings it displays. (It used to be GUI::ItemListModel over a Vector that was local to a
// lambda: ItemListModel keeps only a reference to its container, so the model read freed memory.)
class CatalogListModel final : public GUI::Model {
public:
    static NonnullRefPtr<CatalogListModel> create() { return adopt_ref(*new CatalogListModel); }

    void set_names(Vector<ByteString> names)
    {
        m_names = move(names);
        invalidate();
    }

    virtual int row_count(GUI::ModelIndex const& parent = {}) const override
    {
        return parent.is_valid() ? 0 : static_cast<int>(m_names.size());
    }

    virtual int column_count(GUI::ModelIndex const& = {}) const override { return 1; }

    virtual GUI::Variant data(GUI::ModelIndex const& index, GUI::ModelRole role) const override
    {
        if (role == GUI::ModelRole::TextAlignment)
            return Gfx::TextAlignment::CenterLeft;
        if (role != GUI::ModelRole::Display || !index.is_valid() || index.row() < 0
            || static_cast<size_t>(index.row()) >= m_names.size())
            return {};
        return m_names[static_cast<size_t>(index.row())];
    }

private:
    CatalogListModel() = default;
    Vector<ByteString> m_names;
};

static ExileStore::LoadResult load_catalog_from_disk()
{
    auto file_or_error = Core::File::open(catalog_path, Core::File::OpenMode::Read);
    if (file_or_error.is_error()) {
        return ExileStore::failed_load(ByteString::formatted(
            "The Exile Store catalog could not be opened ({}). Check that the catalog data is installed at {}.",
            file_or_error.error(), catalog_path));
    }
    auto file = file_or_error.release_value();

    auto size_or_error = file->size();
    if (!size_or_error.is_error() && size_or_error.value() > ExileStore::max_catalog_bytes)
        return ExileStore::failed_load("The Exile Store catalog is larger than the supported limit and was not loaded.");

    auto bytes_or_error = file->read_until_eof();
    if (bytes_or_error.is_error()) {
        return ExileStore::failed_load(ByteString::formatted(
            "The Exile Store catalog could not be read ({}).", bytes_or_error.error()));
    }
    auto bytes = bytes_or_error.release_value();
    return ExileStore::parse_catalog(StringView { bytes });
}

static ByteString describe(ExileStore::Entry const& entry)
{
    StringBuilder builder;
    builder.appendff("{}  (version {})\n", entry.name, entry.version);
    builder.appendff("{}\n\n", entry.summary);
    if (!entry.description.is_empty())
        builder.appendff("{}\n\n", entry.description);
    builder.appendff("Publisher: {}\n", entry.publisher);
    builder.appendff("Category: {}\n", entry.category.is_empty() ? ByteString("Uncategorized") : entry.category);
    builder.appendff("License: {}\n", entry.license);
    builder.appendff("Application ID: {}\n", entry.id);

    builder.append("\nPermissions requested:\n"sv);
    if (entry.permissions.is_empty())
        builder.append("  none listed\n"sv);
    for (auto const& permission : entry.permissions)
        builder.appendff("  - {}\n", permission);

    builder.append("\nPackage builds listed in the catalog:\n"sv);
    if (entry.packages.is_empty())
        builder.append("  none listed\n"sv);
    for (auto const& package : entry.packages)
        builder.appendff("  - {}\n", package);

    builder.append("\nInstalled state: not tracked (Exile OS has no package database yet).\n"
                   "Signatures and checksums shown in the catalog are NOT verified by this build."sv);
    return builder.to_byte_string();
}

ErrorOr<int> serenity_main(Main::Arguments arguments)
{
    TRY(Core::System::pledge("stdio recvfd sendfd rpath"));
    TRY(Core::System::unveil(catalog_directory, "r"));
    TRY(Core::System::unveil("/res", "r"));
    TRY(Core::System::unveil(nullptr, nullptr));

    auto app = TRY(GUI::Application::create(arguments));

    auto icon = GUI::Icon::default_icon("exile-store"sv);
    auto window = GUI::Window::construct();
    window->set_title("Exile Store");
    window->set_icon(icon.bitmap_for_size(16));
    window->resize(760, 600);

    auto root = window->set_main_widget<GUI::Widget>();
    root->set_layout<GUI::VerticalBoxLayout>(GUI::Margins { 10, 10, 10, 10 }, 6);

    root->add<GUI::Label>("Exile Store"_string);
    root->add<GUI::Label>("Discover applications for Exile OS. This build is a catalog browser: installing is not available yet."_string);

    auto& toolbar = root->add<GUI::Widget>();
    toolbar.set_layout<GUI::HorizontalBoxLayout>(GUI::Margins {}, 6);
    toolbar.set_fixed_height(26);
    auto& search = toolbar.add<GUI::TextBox>();
    search.set_placeholder("Search applications..."sv);
    auto& refresh_button = toolbar.add<GUI::Button>("Refresh"_string);
    refresh_button.set_fixed_width(80);

    auto& list = root->add<GUI::ListView>();
    auto model = CatalogListModel::create();
    list.set_model(model);

    auto& details = root->add<GUI::Label>("Select an application to view its details."_string);
    details.set_text_alignment(Gfx::TextAlignment::TopLeft);
    details.set_text_wrapping(Gfx::TextWrapping::Wrap);
    details.set_preferred_height(GUI::SpecialDimension::Fit);

    auto& install_button = root->add<GUI::Button>("Install (unavailable)"_string);
    install_button.set_enabled(false);
    install_button.set_fixed_height(26);

    auto& status = root->add<GUI::Label>("Loading catalog..."_string);
    status.set_text_alignment(Gfx::TextAlignment::CenterLeft);

    ExileStore::LoadResult result = load_catalog_from_disk();
    Vector<size_t> visible_entries;

    auto refresh_list = [&] {
        visible_entries = ExileStore::filter_entries(result.entries, search.text());
        Vector<ByteString> names;
        for (auto index : visible_entries)
            names.append(result.entries[index].name);
        model->set_names(move(names));

        if (result.status == ExileStore::LoadStatus::Failed) {
            details.set_text(to_label_text(result.message));
        } else if (result.status == ExileStore::LoadStatus::Empty) {
            details.set_text("The catalog is empty. There is nothing to show yet."_string);
        } else if (visible_entries.is_empty()) {
            details.set_text("No matching applications."_string);
        } else {
            details.set_text("Select an application from the list to see its details."_string);
        }
        // A failed load keeps its own specific error here; it is never replaced by a "0 of 0" count.
        status.set_text(to_label_text(ExileStore::status_text(result, visible_entries.size())));
    };

    search.on_change = [&] {
        refresh_list();
    };

    refresh_button.on_click = [&](auto) {
        result = load_catalog_from_disk();
        refresh_list();
    };

    list.on_selection_change = [&] {
        if (list.selection().is_empty())
            return;
        auto row = list.selection().first().row();
        if (row < 0 || static_cast<size_t>(row) >= visible_entries.size())
            return;
        details.set_text(to_label_text(describe(result.entries[visible_entries[static_cast<size_t>(row)]])));
    };

    refresh_list();

    window->show();
    return app->exec();
}
