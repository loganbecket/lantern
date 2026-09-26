use adw::prelude::*;
use gtk::{gio, glib};

use crate::grid::{PhotoGrid, SortBy};
use crate::viewer::Viewer;

const MIN_CELL: f64 = 96.0;
const MAX_CELL: f64 = 640.0;
const DEFAULT_CELL: f64 = 224.0;

pub fn build(app: &adw::Application, folder: Option<gio::File>) -> adw::ApplicationWindow {
    let grid = PhotoGrid::new(DEFAULT_CELL as i32);

    let open = gtk::Button::from_icon_name("folder-open-symbolic");
    open.set_tooltip_text(Some("Open Folder"));

    let size = gtk::Scale::with_range(gtk::Orientation::Horizontal, MIN_CELL, MAX_CELL, 16.0);
    size.set_value(DEFAULT_CELL);
    size.set_draw_value(false);
    size.set_size_request(180, -1);
    size.set_tooltip_text(Some("Thumbnail size"));

    let title = adw::WindowTitle::new("Lantern", "");

    let sort_menu = gio::Menu::new();
    let by = gio::Menu::new();
    by.append(Some("Date Taken"), Some("win.sort::date"));
    by.append(Some("Name"), Some("win.sort::name"));
    sort_menu.append_section(None, &by);
    let order = gio::Menu::new();
    order.append(Some("Reverse Order"), Some("win.reverse"));
    sort_menu.append_section(None, &order);
    let sort = gtk::MenuButton::builder()
        .icon_name("view-sort-descending-symbolic")
        .tooltip_text("Sort")
        .menu_model(&sort_menu)
        .build();

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    header.pack_start(&open);
    header.pack_end(&size);
    header.pack_end(&sort);

    let empty = adw::StatusPage::builder()
        .icon_name("folder-pictures-symbolic")
        .title("Open a Folder")
        .description("Browse your photos with big thumbnails")
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(grid.widget(), Some("grid"));

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&stack));

    let grid_page = adw::NavigationPage::builder().child(&view).tag("grid").title("Lantern").build();
    let viewer = Viewer::new(grid.store().clone(), grid.thumbs().clone());
    let nav = adw::NavigationView::new();
    nav.add(&grid_page);
    nav.add(viewer.page());

    {
        let nav = nav.clone();
        grid.connect_activate(move |index| {
            viewer.show(index);
            nav.push_by_tag("viewer");
        });
    }

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lantern")
        .default_width(1200)
        .default_height(800)
        .content(&nav)
        .build();

    {
        let grid = grid.clone();
        size.connect_value_changed(move |s| grid.set_cell_size(s.value() as i32));
    }

    add_sort_actions(&window, &grid);

    let show_folder = {
        let grid = grid.clone();
        move |dir: gio::File| {
            title.set_subtitle(&dir.path().unwrap_or_default().display().to_string());
            stack.set_visible_child_name("grid");
            grid.load(dir);
        }
    };

    if let Some(dir) = folder {
        show_folder(dir);
    }

    {
        let window = window.clone();
        open.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Open Folder").modal(true).build();
            let show_folder = show_folder.clone();
            dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Ok(dir) = result {
                    show_folder(dir);
                }
            });
        });
    }

    window
}

/// `win.sort` (date | name) and `win.reverse` (bool), backing the sort menu.
fn add_sort_actions(window: &adw::ApplicationWindow, grid: &PhotoGrid) {
    let sort = gio::SimpleAction::new_stateful("sort", Some(glib::VariantTy::STRING), &"date".to_variant());
    let reverse = gio::SimpleAction::new_stateful("reverse", None, &false.to_variant());

    let apply = {
        let grid = grid.clone();
        let sort = sort.clone();
        let reverse = reverse.clone();
        move || {
            let by = match sort.state().and_then(|s| s.get::<String>()).as_deref() {
                Some("name") => SortBy::Name,
                _ => SortBy::Date,
            };
            let reversed = reverse.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            grid.set_sort(by, reversed);
        }
    };

    {
        let apply = apply.clone();
        sort.connect_change_state(move |action, state| {
            if let Some(state) = state {
                action.set_state(state);
                apply();
            }
        });
    }
    reverse.connect_activate(move |action, _| {
        let current = action.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
        action.set_state(&(!current).to_variant());
        apply();
    });

    window.add_action(&sort);
    window.add_action(&reverse);
}
