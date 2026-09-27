//! Full-size view of one photo, with arrow keys to step through the folder.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::photo::{Entry, PhotoInfo};
use crate::thumbs::{self, CellState, Thumbnails};

/// Decode size when the viewer hasn't been laid out yet.
const FALLBACK_SIZE: u32 = 2048;

/// Thumbnail size in the filmstrip along the bottom.
const STRIP_SIZE: i32 = 88;
/// Padding around each filmstrip cell, matching `.filmstrip > row` in style.css.
const STRIP_PADDING: i32 = 4;

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
    /// Where the photo on screen sits in the store right now, and which
    /// file it is. The store can be re-sorted underneath the viewer (the
    /// date pass finishing, say), so the file is what we trust.
    index: Cell<u32>,
    file: RefCell<Option<gio::File>>,
    /// The filmstrip: every entry of the folder as a small tile, the
    /// current one selected and kept centered.
    strip: gtk::ScrolledWindow,
    strip_selection: gtk::SingleSelection,
    previous: gtk::Button,
    next: gtk::Button,
    rotate_left: gtk::Button,
    rotate_right: gtk::Button,
    /// Where to report what happened (the window shows it as a toast).
    on_message: RefCell<Option<Box<dyn Fn(String)>>>,
    /// Set while the viewer moves the strip's selection itself, so that
    /// doesn't bounce back as a user click.
    syncing: Cell<bool>,
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
        let rotate_button = |icon: &str, tooltip: &str| {
            gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).can_focus(false).build()
        };
        let rotate_left = rotate_button("object-rotate-left-symbolic", "Rotate Left");
        let rotate_right = rotate_button("object-rotate-right-symbolic", "Rotate Right");
        let rotates = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rotates.add_css_class("linked");
        rotates.append(&rotate_left);
        rotates.append(&rotate_right);
        header.pack_start(&rotates);

        let strip_selection = gtk::SingleSelection::builder().model(&store).autoselect(false).build();
        let strip_view = gtk::ListView::builder()
            .model(&strip_selection)
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["filmstrip"])
            .build();
        let strip = gtk::ScrolledWindow::builder()
            .child(&strip_view)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .height_request(STRIP_SIZE + 2 * STRIP_PADDING)
            .build();

        // Previous / next arrows floating over the photo, like any carousel.
        let arrow = |icon: &str, align: gtk::Align| {
            gtk::Button::builder()
                .icon_name(icon)
                .css_classes(["osd", "circular", "large-icons"])
                .halign(align)
                .valign(gtk::Align::Center)
                .margin_start(12)
                .margin_end(12)
                .can_focus(false)
                .build()
        };
        let previous = arrow("go-previous-symbolic", gtk::Align::Start);
        let next = arrow("go-next-symbolic", gtk::Align::End);
        let overlay = gtk::Overlay::builder().child(&picture).build();
        overlay.add_overlay(&previous);
        overlay.add_overlay(&next);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&overlay));
        view.add_bottom_bar(&strip);

        let page = adw::NavigationPage::builder().child(&view).tag("viewer").title("Photo").build();

        let viewer = Rc::new(Self {
            page,
            header,
            title,
            cell: CellState::new(picture, None),
            neighbors: [CellState::new(gtk::Picture::new(), None), CellState::new(gtk::Picture::new(), None)],
            store,
            thumbs,
            index: Cell::new(0),
            file: RefCell::new(None),
            strip,
            strip_selection,
            previous,
            next,
            rotate_left,
            rotate_right,
            on_message: RefCell::new(None),
            syncing: Cell::new(false),
        });

        // Small tiles for the filmstrip, fed by the same loader and cache as
        // the grid, so they are usually already decoded.
        let icon = thumbs::folder_icon(&viewer.page, STRIP_SIZE);
        strip_view.set_factory(Some(&thumbs::cell_factory(&viewer.thumbs, STRIP_SIZE, icon, |item| {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .width_request(STRIP_SIZE)
                .height_request(STRIP_SIZE)
                .css_classes(["skeleton"])
                .build();
            item.set_child(Some(&picture));
            CellState::new(picture, None)
        })));

        for (button, clockwise) in [(&viewer.rotate_left, false), (&viewer.rotate_right, true)] {
            let weak = Rc::downgrade(&viewer);
            button.connect_clicked(move |_| {
                if let Some(viewer) = weak.upgrade() {
                    viewer.rotate(clockwise);
                }
            });
        }

        for (button, delta) in [(&viewer.previous, -1), (&viewer.next, 1)] {
            let weak = Rc::downgrade(&viewer);
            button.connect_clicked(move |_| {
                if let Some(viewer) = weak.upgrade() {
                    viewer.step(delta);
                }
            });
        }

        // A click (or arrow key) in the strip shows that photo.
        let weak = Rc::downgrade(&viewer);
        viewer.strip_selection.connect_selected_notify(move |selection| {
            let Some(viewer) = weak.upgrade() else { return };
            if viewer.syncing.get() {
                return;
            }
            let selected = selection.selected();
            if selected != gtk::INVALID_LIST_POSITION && selected != viewer.index.get() && viewer.photo(selected).is_some() {
                viewer.show(selected);
            }
        });

        // The store was re-sorted or edited: follow our file to its new spot.
        let weak = Rc::downgrade(&viewer);
        viewer.store.connect_items_changed(move |_, _, _, _| {
            if let Some(viewer) = weak.upgrade() {
                viewer.follow_file();
            }
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

    pub fn connect_message(&self, f: impl Fn(String) + 'static) {
        *self.on_message.borrow_mut() = Some(Box::new(f));
    }

    /// Rotate the photo on screen a quarter turn on disk, losslessly, then
    /// show the result everywhere it appears.
    fn rotate(self: &Rc<Self>, clockwise: bool) {
        let Some(path) = self.current().and_then(|p| p.file.path()) else { return };
        self.rotate_left.set_sensitive(false);
        self.rotate_right.set_sensitive(false);

        let (tx, rx) = async_channel::bounded(1);
        let job_path = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::rotate::rotate(&job_path, clockwise));
        });

        let viewer = self.clone();
        glib::spawn_future_local(async move {
            let result = rx.recv().await.unwrap_or_else(|_| Err("rotation failed".into()));
            viewer.rotate_left.set_sensitive(true);
            viewer.rotate_right.set_sensitive(true);
            match result {
                Ok(()) => {
                    // Turn what is already decoded rather than reading the
                    // file again; then rebind this item everywhere it shows.
                    viewer.thumbs.rotate_cached(&path, clockwise);
                    viewer.store.items_changed(viewer.index.get(), 1, 1);
                    viewer.update();
                }
                Err(err) => {
                    if let Some(f) = &*viewer.on_message.borrow() {
                        f(format!("Couldn't rotate: {err}"));
                    }
                }
            }
        });
    }

    /// The photo on screen, only if it is still the one we were showing.
    pub fn current(&self) -> Option<PhotoInfo> {
        let photo = self.photo(self.index.get())?;
        let same = self.file.borrow().as_ref().is_some_and(|f| f.equal(&photo.file));
        same.then_some(photo)
    }

    /// If our file moved to another spot in the store, go there.
    fn follow_file(&self) {
        let Some(file) = self.file.borrow().clone() else { return };
        if self.photo(self.index.get()).is_some_and(|p| p.file.equal(&file)) {
            return;
        }
        let count = self.store.n_items();
        if let Some(index) = (0..count).find(|i| self.photo(*i).is_some_and(|p| p.file.equal(&file))) {
            self.show(index);
        }
    }

    /// The folder changed underneath the viewer (a photo was trashed, moved
    /// or renamed): show whatever photo is at or after this spot now, else
    /// the nearest one before it. Returns false when none is left.
    pub fn refresh(&self) -> bool {
        let index = self.index.get();
        let next = self
            .nearest_photo(index, 1)
            .or_else(|| self.nearest_photo(index.saturating_sub(1), -1));
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
        *self.file.borrow_mut() = self.photo(index).map(|p| p.file);
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

    /// Select `index` in the filmstrip and center it.
    fn sync_strip(&self, index: u32) {
        self.syncing.set(true);
        self.strip_selection.set_selected(index);
        self.syncing.set(false);

        let pitch = (STRIP_SIZE + 2 * STRIP_PADDING) as f64;
        let adjustment = self.strip.hadjustment();
        let page = adjustment.page_size();
        if page > 0.0 {
            let target = index as f64 * pitch + pitch / 2.0 - page / 2.0;
            adjustment.set_value(target.clamp(0.0, (adjustment.upper() - page).max(0.0)));
        }
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
        if !self.strip.has_focus() && self.strip.focus_child().is_none() {
            self.cell.picture.grab_focus();
        }
        self.sync_strip(index);
        self.previous.set_sensitive(index > 0 && self.nearest_photo(index - 1, -1).is_some());
        self.next.set_sensitive(self.nearest_photo(index + 1, 1).is_some());
        // The photo on screen goes first in the decode queue.
        self.thumbs.set_focus(index);

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
