use adw::prelude::*;
use gtk::gio;

use crate::grid::PhotoGrid;

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

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    header.pack_start(&open);
    header.pack_end(&size);

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

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lantern")
        .default_width(1200)
        .default_height(800)
        .content(&view)
        .build();

    {
        let grid = grid.clone();
        size.connect_value_changed(move |s| grid.set_cell_size(s.value() as i32));
    }

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
