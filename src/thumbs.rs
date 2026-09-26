//! Main-thread side of thumbnail loading: hands cells their textures, keeps a
//! bounded in-memory cache, and never touches the disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::{gdk, glib};
use lru::LruCache;

use crate::decode::Decoded;
use crate::loader::{Job, Pool, Reply};

/// Thumbnails are decoded at one of these sizes (in pixels, longest edge) so
/// nudging the size slider reuses what is already decoded.
const BUCKETS: [u32; 4] = [256, 512, 1024, 2048];

/// Upper bound on decoded pixel data held in memory.
const CACHE_BYTES: usize = 512 * 1024 * 1024;

type Key = (PathBuf, u32);

/// One grid cell's picture widget and the thumbnail it is currently showing
/// or waiting for.
pub struct CellState {
    pub picture: gtk::Picture,
    key: RefCell<Option<Key>>,
}

impl CellState {
    pub fn new(picture: gtk::Picture) -> Rc<Self> {
        Rc::new(Self { picture, key: RefCell::new(None) })
    }

    fn show(&self, texture: &gdk::Texture) {
        self.picture.set_paintable(Some(texture));
        self.picture.remove_css_class("skeleton");
        self.picture.remove_css_class("broken");
    }

    fn show_skeleton(&self) {
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.picture.remove_css_class("broken");
        self.picture.add_css_class("skeleton");
    }

    fn show_broken(&self) {
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.picture.remove_css_class("skeleton");
        self.picture.add_css_class("broken");
    }
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
}

impl Thumbnails {
    pub fn new() -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded::<Reply>();
        let this = Rc::new(Self {
            pool: Pool::new(tx),
            cache: RefCell::new(LruCache::unbounded()),
            cache_bytes: RefCell::new(0),
            waiting: RefCell::new(HashMap::new()),
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

    /// Show `path` in `cell` at roughly `size` pixels, now if cached or
    /// once decoded otherwise.
    pub fn request(&self, cell: &Rc<CellState>, path: PathBuf, size: u32) {
        let key = (path, bucket(size));
        *cell.key.borrow_mut() = Some(key.clone());

        if let Some(texture) = self.cache.borrow_mut().get(&key) {
            cell.show(texture);
            return;
        }
        cell.show_skeleton();

        let mut waiting = self.waiting.borrow_mut();
        if let Some(entry) = waiting.get_mut(&key) {
            entry.cells.push(Rc::downgrade(cell));
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.pool.submit(Job { path: key.0.clone(), target: key.1, cancel: cancel.clone() });
        waiting.insert(key, Waiting { cancel, cells: vec![Rc::downgrade(cell)] });
    }

    /// The cell is being recycled; drop its request and cancel the decode if
    /// nobody else wants it.
    pub fn release(&self, cell: &Rc<CellState>) {
        let Some(key) = cell.key.borrow_mut().take() else { return };
        let mut waiting = self.waiting.borrow_mut();
        let Some(entry) = waiting.get_mut(&key) else { return };
        entry.cells.retain(|c| c.upgrade().is_some_and(|c| !Rc::ptr_eq(&c, cell)));
        if entry.cells.is_empty() {
            entry.cancel.store(true, Ordering::Relaxed);
            waiting.remove(&key);
        }
    }

    fn deliver(&self, reply: Reply) {
        let key = (reply.path, reply.target);
        let Some(entry) = self.waiting.borrow_mut().remove(&key) else { return };

        match reply.result {
            Ok(decoded) => {
                let texture = texture_from(decoded);
                self.insert(key.clone(), texture.clone());
                for cell in entry.cells.iter().filter_map(Weak::upgrade) {
                    if cell.key.borrow().as_ref() == Some(&key) {
                        cell.show(&texture);
                    }
                }
            }
            Err(err) => {
                eprintln!("lantern: {}: {err}", key.0.display());
                for cell in entry.cells.iter().filter_map(Weak::upgrade) {
                    if cell.key.borrow().as_ref() == Some(&key) {
                        cell.show_broken();
                    }
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
