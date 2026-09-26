use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::thumbs::{CellState, Thumbnails};

/// File extensions Lantern treats as photos, lowercase.
const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "gif", "tif", "tiff", "bmp", "avif",
];

/// Padding around each cell, matching `gridview > child` in style.css.
const CELL_PADDING: i32 = 6;

/// A scrolling grid of photos from one folder.
///
/// This is a thin handle around the GTK widgets; cloning it is cheap and every
/// clone points at the same grid.
#[derive(Clone)]
pub struct PhotoGrid {
    scrolled: gtk::ScrolledWindow,
    view: gtk::GridView,
    store: gio::ListStore,
    cell_size: Rc<Cell<i32>>,
    thumbs: Rc<Thumbnails>,
}

impl PhotoGrid {
    pub fn new(cell_size: i32) -> Self {
        let store = gio::ListStore::new::<gio::File>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));

        let view = gtk::GridView::builder().model(&selection).min_columns(1).build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();

        let grid = Self {
            scrolled,
            view,
            store,
            cell_size: Rc::new(Cell::new(cell_size)),
            thumbs: Thumbnails::new(),
        };
        grid.view.set_factory(Some(&grid.make_factory()));

        // The viewport width arrives through the horizontal adjustment.
        let weak = grid.clone();
        grid.scrolled.hadjustment().connect_page_size_notify(move |_| weak.update_columns());

        let weak = grid.clone();
        grid.scrolled.vadjustment().connect_value_changed(move |_| weak.update_focus());

        grid
    }

    /// Tell the loader which photo is at the top of the viewport.
    fn update_focus(&self) {
        let row_height = (self.cell_size.get() + 2 * CELL_PADDING) as f64;
        let row = (self.scrolled.vadjustment().value() / row_height).floor() as u32;
        self.thumbs.set_focus(row * self.view.max_columns());
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.scrolled
    }

    pub fn set_cell_size(&self, size: i32) {
        if self.cell_size.replace(size) != size {
            self.update_columns();
            // A new factory makes the view rebuild its visible cells at the new size.
            self.view.set_factory(Some(&self.make_factory()));
        }
    }

    /// Keep `max-columns` at the number of columns that actually fit.
    ///
    /// GtkGridView keeps roughly `max_columns * 30` cells alive around the
    /// visible area, so a generous maximum makes it build and decode far more
    /// cells than are on screen.
    fn update_columns(&self) {
        let width = self.scrolled.hadjustment().page_size() as i32;
        let cell = self.cell_size.get() + 2 * CELL_PADDING;
        let columns = if width > 0 { (width / cell).max(1) } else { 1 };
        if self.view.max_columns() != columns as u32 {
            self.view.set_max_columns(columns as u32);
        }
    }

    /// Replace the grid's contents with the photos in `dir`.
    pub fn load(&self, dir: gio::File) {
        let store = self.store.clone();
        store.remove_all();
        glib::spawn_future_local(async move {
            let mut files = match list_photos(&dir).await {
                Ok(files) => files,
                Err(err) => {
                    eprintln!("lantern: cannot read {}: {err}", dir.uri());
                    return;
                }
            };
            files.sort_by_cached_key(|f| f.basename().unwrap_or_default().to_string_lossy().to_lowercase());
            store.extend_from_slice(&files);
        });
    }

    fn make_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let size = self.cell_size.get();
        let thumbs = self.thumbs.clone();
        let cells: Rc<RefCell<HashMap<gtk::ListItem, Rc<CellState>>>> = Rc::default();

        {
            let cells = cells.clone();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let picture = gtk::Picture::builder()
                    .content_fit(gtk::ContentFit::Contain)
                    .can_shrink(true)
                    .width_request(size)
                    .height_request(size)
                    .css_classes(["skeleton"])
                    .build();
                item.set_child(Some(&picture));
                cells.borrow_mut().insert(item.clone(), CellState::new(picture));
            });
        }

        {
            let cells = cells.clone();
            let thumbs = thumbs.clone();
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let file = item.item().and_downcast::<gio::File>().unwrap();
                let Some(cell) = cells.borrow().get(item).cloned() else { return };
                let name = file.basename().unwrap_or_default();
                cell.picture.set_tooltip_text(Some(&name.to_string_lossy()));
                let Some(path) = file.path() else { return };
                let pixels = size as u32 * cell.picture.scale_factor().max(1) as u32;
                thumbs.request(&cell, path, pixels, item.position());
            });
        }

        {
            let cells = cells.clone();
            let thumbs = thumbs.clone();
            factory.connect_unbind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                if let Some(cell) = cells.borrow().get(item) {
                    thumbs.release(cell);
                }
            });
        }

        factory.connect_teardown(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            cells.borrow_mut().remove(item);
        });

        factory
    }
}

/// List the photo files directly inside `dir`, unsorted.
async fn list_photos(dir: &gio::File) -> Result<Vec<gio::File>, glib::Error> {
    let enumerator = dir
        .enumerate_children_future(
            gio::FILE_ATTRIBUTE_STANDARD_NAME,
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await?;

    let mut files = Vec::new();
    loop {
        let batch = enumerator.next_files_future(256, glib::Priority::DEFAULT).await?;
        if batch.is_empty() {
            break;
        }
        for info in batch {
            let name = info.name();
            let is_photo = name
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| PHOTO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if is_photo {
                files.push(dir.child(name));
            }
        }
    }
    Ok(files)
}
