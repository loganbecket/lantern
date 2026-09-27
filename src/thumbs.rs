//! Main-thread side of thumbnail loading: hands cells their textures, keeps a
//! bounded in-memory cache, and never touches the disk.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::{gdk, glib};
use lru::LruCache;

use crate::decode::Decoded;
use crate::loader::{Job, Kind, Pool, Reply};
use crate::photo::Entry;

/// Thumbnails are decoded at one of these sizes (in pixels, longest edge) so
/// nudging the size slider reuses what is already decoded.
const BUCKETS: [u32; 5] = [256, 512, 1024, 2048, 4096];

/// Cache slot for the embedded preview, whatever its size.
const PREVIEW: u32 = 0;

/// Upper bound on decoded pixel data held in memory.
const CACHE_BYTES: usize = 512 * 1024 * 1024;

type Key = (PathBuf, u32);

/// One cell's picture widget and the thumbnail it is currently showing or
/// waiting for. Used by the grid, the filmstrip and the viewer alike.
pub struct CellState {
    pub picture: gtk::Picture,
    /// Caption shown for folders; only grid tiles have one.
    pub label: Option<gtk::Label>,
    /// How the picture fits its frame when showing a photo (folder icons
    /// always scale down), as set by whoever built the widget.
    fit: gtk::ContentFit,
    /// The full-size request this cell is bound to, if any.
    key: RefCell<Option<Key>>,
    /// Whether the picture holds the real thumbnail (not just a preview).
    sharp: Cell<bool>,
}

impl CellState {
    pub fn new(picture: gtk::Picture, label: Option<gtk::Label>) -> Rc<Self> {
        let fit = picture.content_fit();
        Rc::new(Self { picture, label, fit, key: RefCell::new(None), sharp: Cell::new(false) })
    }

    /// Turn the cell into a folder tile: icon plus name, no thumbnail.
    pub fn show_folder(&self, icon: &gtk::IconPaintable, name: &str) {
        self.picture.set_content_fit(gtk::ContentFit::ScaleDown);
        self.picture.set_paintable(Some(icon));
        self.picture.remove_css_class("skeleton");
        self.picture.remove_css_class("broken");
        if let Some(label) = &self.label {
            label.set_text(name);
            label.set_visible(true);
        }
    }

    fn show(&self, texture: &gdk::Texture, sharp: bool) {
        self.picture.set_content_fit(self.fit);
        self.picture.set_paintable(Some(texture));
        self.picture.remove_css_class("skeleton");
        self.picture.remove_css_class("broken");
        self.sharp.set(sharp);
    }

    fn show_skeleton(&self) {
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.picture.remove_css_class("broken");
        self.picture.add_css_class("skeleton");
        self.sharp.set(false);
    }

    fn show_broken(&self) {
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.picture.remove_css_class("skeleton");
        self.picture.add_css_class("broken");
        self.sharp.set(false);
    }

    fn path(&self) -> Option<PathBuf> {
        self.key.borrow().as_ref().map(|k| k.0.clone())
    }
}

/// The theme's folder icon, sized to sit comfortably inside a cell.
pub fn folder_icon(widget: &impl IsA<gtk::Widget>, cell_size: i32) -> gtk::IconPaintable {
    gtk::IconTheme::for_display(&WidgetExt::display(widget)).lookup_icon(
        "folder",
        &[],
        cell_size / 2,
        widget.scale_factor(),
        gtk::TextDirection::None,
        gtk::IconLookupFlags::empty(),
    )
}

/// A list item factory for a model of `Entry` objects, wired to the loader.
/// `setup` builds one cell's widgets and returns its state; binding,
/// unbinding and teardown are the same for every view that shows entries.
pub fn cell_factory(
    thumbs: &Rc<Thumbnails>,
    size: i32,
    folder_icon: gtk::IconPaintable,
    setup: impl Fn(&gtk::ListItem) -> Rc<CellState> + 'static,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let cells: Rc<RefCell<HashMap<gtk::ListItem, Rc<CellState>>>> = Rc::default();

    {
        let cells = cells.clone();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            cells.borrow_mut().insert(item.clone(), setup(item));
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
                    if let Some(label) = &cell.label {
                        label.set_visible(false);
                    }
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

struct Waiting {
    cancel: Arc<AtomicBool>,
    cells: Vec<Weak<CellState>>,
}

pub struct Thumbnails {
    pool: Pool,
    cache: RefCell<LruCache<Key, gdk::Texture>>,
    cache_bytes: RefCell<usize>,
    waiting: RefCell<HashMap<Key, Waiting>>,
    /// Files known to have no embedded preview, so they aren't re-read
    /// every time their cell scrolls back into view.
    no_preview: RefCell<HashSet<PathBuf>>,
}

/// Only JPEGs carry a cheap EXIF preview; anything else would read 64 KB
/// for nothing.
fn may_have_preview(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
}

impl Thumbnails {
    pub fn new() -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded::<Reply>();
        let this = Rc::new(Self {
            pool: Pool::new(tx),
            cache: RefCell::new(LruCache::unbounded()),
            cache_bytes: RefCell::new(0),
            waiting: RefCell::new(HashMap::new()),
            no_preview: RefCell::new(HashSet::new()),
        });

        let weak = Rc::downgrade(&this);
        glib::spawn_future_local(async move {
            while let Ok(reply) = rx.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this.deliver(reply);
            }
        });

        this
    }

    /// Tell the decode pool which photo is at the top of the viewport so it
    /// works outward from there.
    pub fn set_focus(&self, position: u32) {
        self.pool.set_focus(position);
    }

    /// Drop everything known about `path` (after it changed on disk or went
    /// away, so a new file under the same name never shows the old pixels).
    pub fn forget(&self, path: &Path) {
        let mut cache = self.cache.borrow_mut();
        let mut bytes = self.cache_bytes.borrow_mut();
        for bucket in BUCKETS.iter().chain([PREVIEW].iter()) {
            if let Some(texture) = cache.pop(&(path.to_path_buf(), *bucket)) {
                *bytes -= texture_bytes(&texture);
            }
        }
        self.no_preview.borrow_mut().remove(path);
    }

    /// The largest decoded version of `path` already in memory, at any size,
    /// down to the embedded preview.
    pub fn any_cached(&self, path: &Path) -> Option<gdk::Texture> {
        let mut cache = self.cache.borrow_mut();
        BUCKETS
            .iter()
            .rev()
            .chain([PREVIEW].iter())
            .find_map(|b| cache.get(&(path.to_path_buf(), *b)).cloned())
    }

    /// A cached texture at least as big as `bucket`, smallest first, so a
    /// sharper copy in memory never has to be decoded again at a smaller
    /// size (the GPU scales it down).
    fn cached_at_least(&self, path: &Path, bucket: u32) -> Option<gdk::Texture> {
        let mut cache = self.cache.borrow_mut();
        BUCKETS
            .iter()
            .filter(|b| **b >= bucket)
            .find_map(|b| cache.get(&(path.to_path_buf(), *b)).cloned())
    }

    /// Show `path` in `cell` at roughly `size` pixels, now if cached or
    /// once decoded otherwise. `position` is the photo's index in the grid.
    /// With `placeholder`, the cell shows the skeleton (or the embedded
    /// preview, if that is already in memory) while it waits; otherwise
    /// whatever it shows now stays until the new texture lands.
    pub fn request(&self, cell: &Rc<CellState>, path: PathBuf, size: u32, position: u32, placeholder: bool) {
        let key = (path.clone(), bucket(size));
        *cell.key.borrow_mut() = Some(key.clone());

        if let Some(texture) = self.cached_at_least(&path, key.1) {
            cell.show(&texture, true);
            return;
        }
        if placeholder {
            match self.cache.borrow_mut().get(&(path.clone(), PREVIEW)) {
                Some(preview) => cell.show(preview, false),
                None => cell.show_skeleton(),
            }
        }

        self.enqueue(&key, cell, || Job {
            path: path.clone(),
            kind: Kind::Full,
            target: key.1,
            position,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let preview_key = (path.clone(), PREVIEW);
        let wanted = !cell.sharp.get()
            && may_have_preview(&path)
            && !self.no_preview.borrow().contains(&path)
            && !self.cache.borrow().contains(&preview_key);
        if wanted {
            // Its own flag: the preview and the full decode are waited on
            // by different sets of cells and are canceled separately.
            self.enqueue(&preview_key, cell, || Job {
                path,
                kind: Kind::Preview,
                target: PREVIEW,
                position,
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
    }

    /// Register `cell` as waiting on `key`, submitting the job if nobody
    /// else already asked for it.
    fn enqueue(&self, key: &Key, cell: &Rc<CellState>, job: impl FnOnce() -> Job) {
        let mut waiting = self.waiting.borrow_mut();
        if let Some(entry) = waiting.get_mut(key) {
            entry.cells.push(Rc::downgrade(cell));
            return;
        }
        let job = job();
        let cancel = job.cancel.clone();
        self.pool.submit(job);
        waiting.insert(key.clone(), Waiting { cancel, cells: vec![Rc::downgrade(cell)] });
    }

    /// The cell is being recycled; drop its requests and cancel the decodes
    /// if nobody else wants them.
    pub fn release(&self, cell: &Rc<CellState>) {
        let Some(key) = cell.key.borrow_mut().take() else { return };
        let mut waiting = self.waiting.borrow_mut();
        for key in [key.clone(), (key.0, PREVIEW)] {
            let Some(entry) = waiting.get_mut(&key) else { continue };
            entry.cells.retain(|c| c.upgrade().is_some_and(|c| !Rc::ptr_eq(&c, cell)));
            if entry.cells.is_empty() {
                entry.cancel.store(true, Ordering::Relaxed);
                waiting.remove(&key);
            }
        }
    }

    fn deliver(&self, reply: Reply) {
        let key = (reply.path, reply.target);
        let Some(entry) = self.waiting.borrow_mut().remove(&key) else { return };
        let cells = entry.cells.iter().filter_map(Weak::upgrade).filter(|c| c.path().as_deref() == Some(&key.0));

        match (reply.kind, reply.result) {
            (Kind::Full, Ok(decoded)) => {
                let texture = texture_from(decoded);
                self.insert(key.clone(), texture.clone());
                for cell in cells {
                    cell.show(&texture, true);
                }
            }
            (Kind::Preview, Ok(decoded)) => {
                let texture = texture_from(decoded);
                self.insert(key.clone(), texture.clone());
                for cell in cells.filter(|c| !c.sharp.get()) {
                    cell.show(&texture, false);
                }
            }
            (Kind::Preview, Err(_)) => {
                // No embedded preview: the skeleton stays until the full decode.
                self.no_preview.borrow_mut().insert(key.0);
            }
            (Kind::Full, Err(err)) => {
                eprintln!("lantern: {}: {err}", key.0.display());
                for cell in cells {
                    cell.show_broken();
                }
            }
        }
    }

    fn insert(&self, key: Key, texture: gdk::Texture) {
        let mut cache = self.cache.borrow_mut();
        let mut bytes = self.cache_bytes.borrow_mut();
        *bytes += texture_bytes(&texture);
        cache.put(key, texture);
        while *bytes > CACHE_BYTES {
            let Some((_, old)) = cache.pop_lru() else { break };
            *bytes -= texture_bytes(&old);
        }
    }
}

fn bucket(size: u32) -> u32 {
    BUCKETS.into_iter().find(|b| *b >= size).unwrap_or(BUCKETS[BUCKETS.len() - 1])
}

fn texture_bytes(texture: &gdk::Texture) -> usize {
    texture.width() as usize * texture.height() as usize * 4
}

fn texture_from(decoded: Decoded) -> gdk::Texture {
    let stride = decoded.width as usize * 4;
    gdk::MemoryTexture::new(
        decoded.width as i32,
        decoded.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(decoded.rgba),
        stride,
    )
    .upcast()
}
