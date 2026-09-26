use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::actions;
use crate::grid::{PhotoGrid, SortBy};
use crate::photo::PhotoInfo;
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
    header.pack_end(&file_buttons());

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
    viewer.header().pack_end(&file_buttons());
    let nav = adw::NavigationView::new();
    nav.add(&grid_page);
    nav.add(viewer.page());

    {
        let nav = nav.clone();
        let viewer = viewer.clone();
        grid.connect_activate(move |index| {
            viewer.show(index);
            nav.push_by_tag("viewer");
        });
    }

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lantern")
        .default_width(1200)
        .default_height(800)
        .content(&toasts)
        .build();

    {
        let grid = grid.clone();
        size.connect_value_changed(move |s| grid.set_cell_size(s.value() as i32));
    }

    add_sort_actions(&window, &grid);
    add_file_actions(&window, &grid, &viewer, &nav, &toasts);

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

/// Trash, Download, Move/Rename, as a linked group for a header bar.
fn file_buttons() -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    group.add_css_class("linked");
    for (icon, tooltip, action) in [
        ("folder-download-symbolic", "Copy to Downloads (Ctrl+D)", "win.download"),
        ("send-to-symbolic", "Move or Rename (F2)", "win.move"),
        ("user-trash-symbolic", "Move to Trash (Delete)", "win.delete"),
    ] {
        let button = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).action_name(action).build();
        group.append(&button);
    }
    group
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

/// `win.delete`, `win.download`, `win.move`: act on the selection in the
/// grid, or on the photo on screen in the viewer.
fn add_file_actions(
    window: &adw::ApplicationWindow,
    grid: &PhotoGrid,
    viewer: &Rc<Viewer>,
    nav: &adw::NavigationView,
    toasts: &adw::ToastOverlay,
) {
    let delete = gio::SimpleAction::new("delete", None);
    let download = gio::SimpleAction::new("download", None);
    let mv = gio::SimpleAction::new("move", None);
    let all = [delete.clone(), download.clone(), mv.clone()];

    let in_viewer = {
        let nav = nav.clone();
        move || nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("viewer")
    };

    // Whose files: the viewer's one photo, or the grid's selection.
    let targets = {
        let grid = grid.clone();
        let viewer = viewer.clone();
        let in_viewer = in_viewer.clone();
        move || -> Vec<PhotoInfo> {
            if in_viewer() { viewer.current().into_iter().collect() } else { grid.selected() }
        }
    };

    // Buttons light up only when there is something to act on.
    let refresh = {
        let targets = targets.clone();
        move || {
            let enabled = !targets().is_empty();
            for action in &all {
                action.set_enabled(enabled);
            }
        }
    };
    refresh();
    {
        let refresh = refresh.clone();
        grid.connect_selection_changed(move || refresh());
    }
    {
        let refresh = refresh.clone();
        nav.connect_visible_page_notify(move |_| refresh());
    }

    // After files change under us: fix up the viewer, then report.
    let finish = {
        let grid = grid.clone();
        let viewer = viewer.clone();
        let nav = nav.clone();
        let toasts = toasts.clone();
        let in_viewer = in_viewer.clone();
        let refresh = refresh.clone();
        move |message: String| {
            if in_viewer() && !viewer.refresh() {
                nav.pop();
            }
            refresh();
            let _ = &grid;
            toasts.add_toast(adw::Toast::new(&message));
        }
    };

    {
        let grid = grid.clone();
        let targets = targets.clone();
        let finish = finish.clone();
        delete.connect_activate(move |_, _| {
            let files: Vec<gio::File> = targets().into_iter().map(|p| p.file).collect();
            let grid = grid.clone();
            let finish = finish.clone();
            glib::spawn_future_local(async move {
                let outcome = actions::trash(files).await;
                let done: Vec<gio::File> = outcome.iter().filter(|(_, r)| r.is_ok()).map(|(f, _)| f.clone()).collect();
                grid.remove_files(&done);
                finish(summarize(&outcome, "Moved", "to Trash"));
            });
        });
    }

    {
        let targets = targets.clone();
        let finish = finish.clone();
        download.connect_activate(move |_, _| {
            let files: Vec<gio::File> = targets().into_iter().map(|p| p.file).collect();
            let finish = finish.clone();
            glib::spawn_future_local(async move {
                let outcome = actions::download(files).await;
                finish(summarize(&outcome, "Copied", "to Downloads"));
            });
        });
    }

    {
        let window = window.clone();
        let grid = grid.clone();
        mv.connect_activate(move |_, _| {
            let photos = targets();
            let grid = grid.clone();
            let finish = finish.clone();
            let dialog = gtk::FileDialog::builder().modal(true).build();
            if let Some(dir) = grid.dir() {
                dialog.set_initial_folder(Some(&dir));
            }

            if let [photo] = photos.as_slice() {
                // One photo: pick a folder and a name in one go.
                dialog.set_title("Move or Rename");
                dialog.set_initial_name(Some(&photo.name));
                let source = photo.file.clone();
                dialog.save(Some(&window), gio::Cancellable::NONE, move |result| {
                    let Ok(target) = result else { return };
                    glib::spawn_future_local(async move {
                        let result = actions::move_to(&source, target).await;
                        match &result {
                            Ok(moved) if moved.parent().is_some_and(|p| grid.dir().is_some_and(|d| d.equal(&p))) => {
                                grid.replace_file(&source, moved.clone());
                            }
                            Ok(_) => grid.remove_files(&[source.clone()]),
                            Err(_) => {}
                        }
                        finish(summarize(&[(source, result)], "Moved", ""));
                    });
                });
            } else {
                dialog.set_title("Move to Folder");
                let files: Vec<gio::File> = photos.into_iter().map(|p| p.file).collect();
                dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |result| {
                    let Ok(folder) = result else { return };
                    glib::spawn_future_local(async move {
                        let outcome = actions::move_into(files, folder.clone()).await;
                        if !grid.dir().is_some_and(|d| d.equal(&folder)) {
                            let done: Vec<gio::File> = outcome.iter().filter(|(_, r)| r.is_ok()).map(|(f, _)| f.clone()).collect();
                            grid.remove_files(&done);
                        }
                        finish(summarize(&outcome, "Moved", ""));
                    });
                });
            }
        });
    }

    window.add_action(&delete);
    window.add_action(&download);
    window.add_action(&mv);
    let app = window.application().unwrap();
    app.set_accels_for_action("win.delete", &["Delete"]);
    app.set_accels_for_action("win.download", &["<Control>d"]);
    app.set_accels_for_action("win.move", &["F2"]);
}

/// "Moved 3 photos to Trash", or the first error if anything failed.
fn summarize(outcome: &[(gio::File, Result<gio::File, String>)], verb: &str, suffix: &str) -> String {
    if let Some((file, Err(err))) = outcome.iter().find(|(_, r)| r.is_err()) {
        let name = file.basename().unwrap_or_default().to_string_lossy().into_owned();
        return format!("Couldn't move {name}: {err}");
    }
    let count = outcome.len();
    let what = match (count, outcome.first()) {
        (1, Some((_, Ok(target)))) => target.basename().unwrap_or_default().to_string_lossy().into_owned(),
        _ => format!("{count} photos"),
    };
    format!("{verb} {what} {suffix}").trim_end().to_string()
}
