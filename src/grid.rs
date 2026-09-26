use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

/// File extensions Lantern treats as photos, lowercase.
const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "gif", "tif", "tiff", "bmp", "avif",
];

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
}

impl PhotoGrid {
    pub fn new(cell_size: i32) -> Self {
        let store = gio::ListStore::new::<gio::File>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));

        let view = gtk::GridView::builder()
            .model(&selection)
            .min_columns(1)
            .max_columns(64)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();

        let grid = Self { scrolled, view, store, cell_size: Rc::new(Cell::new(cell_size)) };
        grid.view.set_factory(Some(&grid.make_factory()));
        grid
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.scrolled
    }

    pub fn set_cell_size(&self, size: i32) {
        if self.cell_size.replace(size) != size {
            // A new factory makes the view rebuild its visible cells at the new size.
            self.view.set_factory(Some(&self.make_factory()));
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

        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let skeleton = gtk::Box::new(gtk::Orientation::Vertical, 0);
            skeleton.add_css_class("skeleton");
            skeleton.set_size_request(size, size);
            item.set_child(Some(&skeleton));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let file = item.item().and_downcast::<gio::File>().unwrap();
            let name = file.basename().unwrap_or_default();
            item.child().unwrap().set_tooltip_text(Some(&name.to_string_lossy()));
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
