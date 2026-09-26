//! Full-size view of one photo, with arrow keys to step through the folder.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::photo::{Entry, PhotoInfo};
use crate::thumbs::{CellState, Thumbnails};

/// Decode size when the viewer hasn't been laid out yet.
const FALLBACK_SIZE: u32 = 2048;

pub struct Viewer {
    page: adw::NavigationPage,
    header: adw::HeaderBar,
    title: adw::WindowTitle,
    /// The picture on screen, driven by the shared thumbnail loader.
    cell: Rc<CellState>,
    /// Hidden pictures that warm the cache for the previous and next photo.
    neighbors: [Rc<CellState>; 2],
    store: gio::ListStore,
    thumbs: Rc<Thumbnails>,
    index: Cell<u32>,
}

impl Viewer {
    pub fn new(store: gio::ListStore, thumbs: Rc<Thumbnails>) -> Rc<Self> {
        // Focusable so the arrow keys land here rather than in the grid
        // underneath.
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .build();

        let title = adw::WindowTitle::new("", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&picture));

        let page = adw::NavigationPage::builder().child(&view).tag("viewer").title("Photo").build();

        let viewer = Rc::new(Self {
            page,
            header,
            title,
            cell: CellState::new(picture, gtk::Label::new(None)),
            neighbors: [
                CellState::new(gtk::Picture::new(), gtk::Label::new(None)),
                CellState::new(gtk::Picture::new(), gtk::Label::new(None)),
            ],
            store,
            thumbs,
            index: Cell::new(0),
        });

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&viewer);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(viewer) = weak.upgrade() else { return glib::Propagation::Proceed };
            match key {
                gdk::Key::Left | gdk::Key::Up | gdk::Key::Page_Up | gdk::Key::BackSpace => viewer.step(-1),
                gdk::Key::Right | gdk::Key::Down | gdk::Key::Page_Down | gdk::Key::space => viewer.step(1),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        viewer.page.add_controller(keys);

        // Re-request at the real size once the page has been laid out.
        let weak = Rc::downgrade(&viewer);
        viewer.cell.picture.connect_map(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.update();
            }
        });

        // Drop the big textures' requests when leaving the viewer.
        let weak = Rc::downgrade(&viewer);
        viewer.page.connect_hidden(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.thumbs.release(&viewer.cell);
                for n in &viewer.neighbors {
                    viewer.thumbs.release(n);
                }
            }
        });

        viewer
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    pub fn header(&self) -> &adw::HeaderBar {
        &self.header
    }

    /// The photo on screen.
    pub fn current(&self) -> Option<PhotoInfo> {
        self.photo(self.index.get())
    }

    /// The folder changed underneath the viewer (a photo was trashed, moved
    /// or renamed): show whatever photo is at or after this spot now, else
    /// the nearest one before it. Returns false when none is left.
    pub fn refresh(&self) -> bool {
        let index = self.index.get();
        let next = self.nearest_photo(index, 1).or_else(|| self.nearest_photo(index, -1));
        match next {
            Some(index) => {
                self.show(index);
                true
            }
            None => false,
        }
    }

    /// Show the photo at `index` in the grid's current order.
    pub fn show(&self, index: u32) {
        self.index.set(index);
        self.update();
    }

    fn step(&self, delta: i32) {
        let from = self.index.get() as i64 + delta as i64;
        if from >= 0 {
            if let Some(index) = self.nearest_photo(from as u32, delta) {
                self.show(index);
            }
        }
    }

    /// The first photo at or after `from` (`direction` 1) or at or before
    /// it (`direction` -1), skipping folder tiles.
    fn nearest_photo(&self, from: u32, direction: i32) -> Option<u32> {
        let count = self.store.n_items() as i64;
        let mut index = from as i64;
        while (0..count).contains(&index) {
            if self.photo(index as u32).is_some() {
                return Some(index as u32);
            }
            index += direction as i64;
        }
        None
    }

    fn photo(&self, index: u32) -> Option<PhotoInfo> {
        let object = self.store.item(index).and_downcast::<glib::BoxedAnyObject>()?;
        let photo = object.borrow::<Entry>().photo().cloned();
        photo
    }

    fn target_size(&self) -> u32 {
        let picture = &self.cell.picture;
        let longest = picture.width().max(picture.height());
        let scale = picture.scale_factor().max(1) as u32;
        if longest > 0 { longest as u32 * scale } else { FALLBACK_SIZE }
    }

    fn update(&self) {
        let index = self.index.get();
        let Some(photo) = self.photo(index) else { return };
        let Some(path) = photo.file.path() else { return };

        self.title.set_title(&photo.name);
        self.title.set_subtitle(&format!("{} of {}", index + 1, self.store.n_items()));
        self.cell.picture.grab_focus();

        let size = self.target_size();

        // Put whatever thumbnail is already decoded on screen right away, so
        // stepping through photos feels instant while the full size loads.
        self.thumbs.release(&self.cell);
        let placeholder = self.thumbs.any_cached(&path).is_none();
        if let Some(texture) = self.thumbs.any_cached(&path) {
            self.cell.picture.set_paintable(Some(&texture));
        }
        self.thumbs.request(&self.cell, path, size, index, placeholder);

        for (cell, neighbor) in self.neighbors.iter().zip([index.wrapping_sub(1), index + 1]) {
            self.thumbs.release(cell);
            if let Some(path) = self.photo(neighbor).and_then(|p| p.file.path()) {
                self.thumbs.request(cell, path, size, neighbor, true);
            }
        }
    }
}
