@GUI::Widget {
    fill_with_background_color: true
    layout: @GUI::VerticalBoxLayout {
        spacing: 2
    }

    @GUI::ToolbarContainer {
        name: "toolbar_container"

        @GUI::Toolbar {
            name: "main_toolbar"
            collapsible: true
            grouped: true
        }

        @GUI::Toolbar {
            name: "breadcrumb_toolbar"

            @GUI::Label {
                text: "Location: "
                autosize: true
            }

            @GUI::PathBreadcrumbbar {
                name: "breadcrumbbar"
            }
        }
    }

    @GUI::Widget {
        name: "navigator_bar"
        preferred_height: "shrink"
        layout: @GUI::HorizontalBoxLayout {
            margins: [8, 6]
            spacing: 6
        }

        @GUI::Button {
            name: "workspace_button"
            text: "Workspace"
        }

        @GUI::Button {
            name: "projects_button"
            text: "Projects"
        }

        @GUI::Button {
            name: "media_button"
            text: "Media"
        }

        @GUI::Button {
            name: "all_files_button"
            text: "All Files"
        }

        @GUI::Button {
            name: "vault_button"
            text: "Vault (Planned)"
        }
    }

    @GUI::HorizontalSplitter {
        name: "splitter"

        @GUI::TreeView {
            name: "tree_view"
            preferred_width: 175
        }
    }

    @GUI::Statusbar {
        name: "statusbar"

        @GUI::Progressbar {
            name: "progressbar"
            text: "Generating thumbnails: "
            visible: false
        }
    }
}
