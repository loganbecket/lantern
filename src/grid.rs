use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::photo::{self, Entry, PhotoInfo};
use crate::thumbs::{CellState, Thumbnails};

/// File extensions Lantern treats as photos, lowercase.
const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "gif", "tif", "tiff", "bmp", "avif",
];

/// Padding around each cell, matching `gridview > child` in style.css.
const CELL_PADDING: i32 = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Date,
    Name,
}

/// A scrolling grid of the subfolders and photos in one folder.
///
/// This is a thin handle around the GTK widgets; cloning it is cheap and every
/// clone points at the same grid.
#[derive(Clone)]
pub struct PhotoGrid {
    scrolled: gtk::ScrolledWindow,
    view: gtk::GridView,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    dir: Rc<RefCell<Option<gio::File>>>,
    cell_size: Rc<Cell<i32>>,
    thumbs: Rc<Thumbnails>,
    /// Subfolders of the open folder, sorted by name.
    folders: Rc<RefCell<Vec<Entry>>>,
    /// Photos in the open folder, unsorted.
    photos: Rc<RefCell<Vec<PhotoInfo>>>,
    /// Bumped on every load so late results from an old folder are dropped.
    generation: Rc<Cell<u32>>,
    sort_by: Rc<Cell<SortBy>>,
    reverse: Rc<Cell<bool>>,
    /// Called once a folder's listing is in, with whether it had anything.
    on_loaded: Rc<RefCell<Option<Box<dyn Fn(bool)>>>>,
}

impl PhotoGrid {
    pub fn new(cell_size: i32) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));

        let view = gtk::GridView::builder()
            .model(&selection)
            .min_columns(1)
            .enable_rubberband(true)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();

        let grid = Self {
            scrolled,
            view,
            store,
            selection,
            dir: Rc::default(),
            cell_size: Rc::new(Cell::new(cell_size)),
            thumbs: Thumbnails::new(),
            folders: Rc::default(),
            photos: Rc::default(),
            generation: Rc::default(),
            sort_by: Rc::new(Cell::new(SortBy::Date)),
            reverse: Rc::new(Cell::new(false)),
            on_loaded: Rc::default(),
        };
        grid.view.set_factory(Some(&grid.make_factory()));

        // The viewport width arrives through the horizontal adjustment.
        let weak = grid.clone();
        grid.scrolled.hadjustment().connect_page_size_notify(move |_| weak.update_columns());

        let weak = grid.clone();
        grid.scrolled.vadjustment().connect_value_changed(move |_| weak.update_focus());

        grid
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.scrolled
    }

    /// The entries in their current order.
    pub fn store(&self) -> &gio::ListStore {
        &self.store
    }

    pub fn thumbs(&self) -> &Rc<Thumbnails> {
        &self.thumbs
    }

    /// Called with the index of an entry the user opened (double-click or Enter).
    pub fn connect_activate(&self, f: impl Fn(u32) + 'static) {
        self.view.connect_activate(move |_, position| f(position));
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        self.selection.connect_selection_changed(move |_, _, _| f());
    }

    pub fn connect_loaded(&self, f: impl Fn(bool) + 'static) {
        *self.on_loaded.borrow_mut() = Some(Box::new(f));
    }

    /// The open folder, if any.
    pub fn dir(&self) -> Option<gio::File> {
        self.dir.borrow().clone()
    }

    pub fn entry_at(&self, index: u32) -> Option<Entry> {
        let object = self.store.item(index).and_downcast::<glib::BoxedAnyObject>()?;
        let entry = object.borrow::<Entry>().clone();
        Some(entry)
    }

    /// The selected photos in grid order; selected folders are ignored.
    pub fn selected(&self) -> Vec<PhotoInfo> {
        let bitset = self.selection.selection();
        let mut photos = Vec::new();
        if let Some((mut iter, first)) = gtk::BitsetIter::init_first(&bitset) {
            photos.extend(self.entry_at(first).and_then(|e| e.photo().cloned()));
            while let Some(index) = iter.next() {
                photos.extend(self.entry_at(index).and_then(|e| e.photo().cloned()));
            }
        }
        photos
    }

    /// Take photos out of the grid after they were trashed or moved away.
    pub fn remove_files(&self, files: &[gio::File]) {
        let gone = |f: &gio::File| files.iter().any(|g| g.equal(f));
        self.photos.borrow_mut().retain(|p| !gone(&p.file));
        for index in (0..self.store.n_items()).rev() {
            if self.entry_at(index).is_some_and(|e| e.photo().is_some() && gone(e.file())) {
                self.store.remove(index);
            }
        }
    }

    /// A photo was renamed in place; keep its spot, update its identity.
    pub fn replace_file(&self, old: &gio::File, new: gio::File) {
        let name = new.basename().unwrap_or_default().to_string_lossy().into_owned();
        let mut photos = self.photos.borrow_mut();
        let Some(photo) = photos.iter_mut().find(|p| p.file.equal(old)) else { return };
        photo.file = new;
        photo.name = name;
        let updated = Entry::Photo(photo.clone());
        drop(photos);
        for index in 0..self.store.n_items() {
            if self.entry_at(index).is_some_and(|e| e.photo().is_some() && e.file().equal(old)) {
                self.store.splice(index, 1, &[glib::BoxedAnyObject::new(updated)]);
                break;
            }
        }
    }

    pub fn set_cell_size(&self, size: i32) {
        if self.cell_size.replace(size) != size {
            self.update_columns();
            // A new factory makes the view rebuild its visible cells at the new size.
            self.view.set_factory(Some(&self.make_factory()));
        }
    }

    pub fn set_sort(&self, sort_by: SortBy, reverse: bool) {
        self.sort_by.set(sort_by);
        self.reverse.set(reverse);
        self.apply_sort();
        self.scrolled.vadjustment().set_value(0.0);
    }

    /// Tell the loader which photo is at the top of the viewport.
    fn update_focus(&self) {
        let row_height = (self.cell_size.get() + 2 * CELL_PADDING) as f64;
        let row = (self.scrolled.vadjustment().value() / row_height).floor() as u32;
        self.thumbs.set_focus(row * self.view.max_columns());
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

    /// Replace the grid's contents with the subfolders and photos in `dir`.
    ///
    /// The folder listing (names and modified times) is cheap, so the grid
    /// fills right away in that order. Dates taken need a read of every
    /// file's header; those come in the background and the grid re-sorts
    /// once, when they are all in.
    pub fn load(&self, dir: gio::File) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        *self.dir.borrow_mut() = Some(dir.clone());
        self.folders.borrow_mut().clear();
        self.photos.borrow_mut().clear();
        self.store.remove_all();
        self.scrolled.vadjustment().set_value(0.0);

        let grid = self.clone();
        glib::spawn_future_local(async move {
            let (folders, photos) = match list_entries(&dir).await {
                Ok(listing) => listing,
                Err(err) => {
                    eprintln!("lantern: cannot read {}: {err}", dir.uri());
                    return;
                }
            };
            if grid.generation.get() != generation {
                return;
            }
            let has_entries = !folders.is_empty() || !photos.is_empty();
            *grid.folders.borrow_mut() = folders;
            *grid.photos.borrow_mut() = photos.clone();
            grid.apply_sort();
            if let Some(on_loaded) = &*grid.on_loaded.borrow() {
                on_loaded(has_entries);
            }

            let started = std::time::Instant::now();
            let taken = read_taken_dates(&photos).await;
            if grid.generation.get() != generation {
                return;
            }
            if std::env::var_os("LANTERN_DEBUG").is_some() {
                let found = taken.iter().filter(|t| t.is_some()).count();
                eprintln!("dates: {found} of {} in {} ms", taken.len(), started.elapsed().as_millis());
            }
            let mut current = grid.photos.borrow_mut();
            for (photo, taken) in current.iter_mut().zip(taken) {
                photo.taken = taken;
            }
            drop(current);
            if grid.sort_by.get() == SortBy::Date {
                grid.resort();
            }
        });
    }

    /// Re-sort after dates arrive, leaving the view alone if nothing moved.
    /// Modified times usually match dates taken, so this is the common case
    /// and the user never sees a jump.
    fn resort(&self) {
        let sorted = self.sorted();
        let unchanged = sorted.len() as u32 == self.store.n_items()
            && sorted.iter().enumerate().all(|(i, e)| {
                self.entry_at(i as u32).is_some_and(|current| current.file().equal(e.file()))
            });
        if !unchanged {
            self.replace_items(sorted);
        }
    }

    /// Sort the known entries and replace the model in one step.
    fn apply_sort(&self) {
        let sorted = self.sorted();
        self.replace_items(sorted);
    }

    /// Folders first, by name; then photos in the chosen order.
    fn sorted(&self) -> Vec<Entry> {
        let mut photos = self.photos.borrow().clone();
        match self.sort_by.get() {
            SortBy::Date => photos.sort_by(|a, b| a.when().cmp(&b.when()).then_with(|| a.name.cmp(&b.name))),
            SortBy::Name => photos.sort_by_cached_key(|p| p.name.to_lowercase()),
        }
        if self.reverse.get() {
            photos.reverse();
        }
        let mut entries = self.folders.borrow().clone();
        entries.extend(photos.into_iter().map(Entry::Photo));
        entries
    }

    fn replace_items(&self, entries: Vec<Entry>) {
        if std::env::var_os("LANTERN_DEBUG").is_some() {
            let folders = entries.iter().filter(|e| e.photo().is_none()).count();
            eprintln!("entries: {folders} folders, {} photos", entries.len() - folders);
            for p in entries.iter().filter_map(Entry::photo).take(3) {
                eprintln!("sorted: {} taken={:?} modified={}", p.name, p.taken, p.modified);
            }
        }
        let items: Vec<glib::BoxedAnyObject> = entries.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
    }

    fn make_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let size = self.cell_size.get();
        let thumbs = self.thumbs.clone();
        let cells: Rc<RefCell<HashMap<gtk::ListItem, Rc<CellState>>>> = Rc::default();
        let folder_icon = folder_icon(&self.view, size);

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
                let label = gtk::Label::builder()
                    .css_classes(["folder-name"])
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(1)
                    .hexpand(true)
                    .valign(gtk::Align::End)
                    .visible(false)
                    .build();
                let overlay = gtk::Overlay::builder().child(&picture).build();
                overlay.add_overlay(&label);
                item.set_child(Some(&overlay));
                cells.borrow_mut().insert(item.clone(), CellState::new(picture, label));
            });
        }

        {
            let cells = cells.clone();
            let thumbs = thumbs.clone();
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let object = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
                let entry = object.borrow::<Entry>();
                let Some(cell) = cells.borrow().get(item).cloned() else { return };
                cell.picture.set_tooltip_text(Some(entry.name()));
                match &*entry {
                    Entry::Folder { name, .. } => {
                        thumbs.release(&cell);
                        cell.show_folder(&folder_icon, name);
                    }
                    Entry::Photo(photo) => {
                        cell.label.set_visible(false);
                        let Some(path) = photo.file.path() else { return };
                        let pixels = size as u32 * cell.picture.scale_factor().max(1) as u32;
                        thumbs.request(&cell, path, pixels, item.position(), true);
                    }
                }
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

/// The theme's folder icon, sized to sit comfortably inside a cell.
fn folder_icon(widget: &impl IsA<gtk::Widget>, cell_size: i32) -> gtk::IconPaintable {
    gtk::IconTheme::for_display(&WidgetExt::display(widget)).lookup_icon(
        "folder",
        &[],
        cell_size / 2,
        widget.scale_factor(),
        gtk::TextDirection::None,
        gtk::IconLookupFlags::empty(),
    )
}

/// List the subfolders and photo files directly inside `dir`, skipping
/// hidden entries. Folders come back sorted; photos don't, and have no
/// dates taken yet.
async fn list_entries(dir: &gio::File) -> Result<(Vec<Entry>, Vec<PhotoInfo>), glib::Error> {
    let attributes = [
        gio::FILE_ATTRIBUTE_STANDARD_NAME.as_str(),
        gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
        gio::FILE_ATTRIBUTE_STANDARD_IS_HIDDEN.as_str(),
        gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(),
    ]
    .join(",");
    let enumerator = dir
        .enumerate_children_future(&attributes, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT)
        .await?;

    let mut folders = Vec::new();
    let mut photos = Vec::new();
    loop {
        let batch = enumerator.next_files_future(256, glib::Priority::DEFAULT).await?;
        if batch.is_empty() {
            break;
        }
        for info in batch {
            if info.is_hidden() {
                continue;
            }
            let name = info.name();
            if info.file_type() == gio::FileType::Directory {
                folders.push(Entry::Folder { file: dir.child(&name), name: name.to_string_lossy().into_owned() });
                continue;
            }
            let is_photo = name
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| PHOTO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if is_photo {
                photos.push(PhotoInfo {
                    file: dir.child(&name),
                    name: name.to_string_lossy().into_owned(),
                    modified: info.modification_date_time().map_or(0, |t| t.to_unix()),
                    taken: None,
                });
            }
        }
    }
    folders.sort_by_cached_key(|f| f.name().to_lowercase());
    Ok((folders, photos))
}

/// Opening a file on a network share can cost ~100 ms, so a date pass over a
/// big folder there would take minutes and starve the thumbnails. The pass
/// times its first few reads and gives up when they are this slow.
const SLOW_READ_MS: u128 = 20;
const PROBE_FILES: usize = 12;

/// Read every photo's date taken on a few threads, off the main loop.
///
/// Returns all `None` when the folder is too slow to be worth it (see
/// `SLOW_READ_MS`); modified times then stand in for dates taken.
async fn read_taken_dates(photos: &[PhotoInfo]) -> Vec<Option<i64>> {
    let paths: Vec<Option<std::path::PathBuf>> = photos.iter().map(|p| p.file.path()).collect();
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let mut taken = vec![None; paths.len()];

        let probe = paths.len().min(PROBE_FILES);
        let started = std::time::Instant::now();
        for (slot, path) in taken.iter_mut().zip(&paths).take(probe) {
            *slot = path.as_deref().and_then(photo::read_taken);
        }
        if probe > 0 && started.elapsed().as_millis() / probe as u128 > SLOW_READ_MS {
            if std::env::var_os("LANTERN_DEBUG").is_some() {
                eprintln!("dates: folder is slow to read, keeping modified times");
            }
            let _ = tx.send_blocking(vec![None; paths.len()]);
            return;
        }

        let rest = &paths[probe..];
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        let chunk = rest.len().div_ceil(threads).max(1);
        let read: Vec<Option<i64>> = std::thread::scope(|scope| {
            let workers: Vec<_> = rest
                .chunks(chunk)
                .map(|chunk| scope.spawn(move || chunk.iter().map(|p| p.as_deref().and_then(photo::read_taken)).collect::<Vec<_>>()))
                .collect();
            workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
        });
        taken[probe..].copy_from_slice(&read);
        let _ = tx.send_blocking(taken);
    });
    rx.recv().await.unwrap_or_default()
}
