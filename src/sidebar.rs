//! The folder tree in the sidebar: Home and the whole filesystem, expanding
//! one level at a time as the user opens folders.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

/// One row of the tree: a folder, the name to show for it, and an icon
/// for the top-level places (subfolders all get the plain folder icon).
#[derive(Clone, Debug)]
struct Node {
    file: gio::File,
    name: String,
    icon: Option<gio::Icon>,
}

impl Node {
    fn folder(file: gio::File, name: String) -> Self {
        Self { file, name, icon: None }
    }

    fn place(path: impl AsRef<std::path::Path>, name: &str, icon: &str) -> Self {
        Self { file: gio::File::for_path(path), name: name.into(), icon: Some(gio::ThemedIcon::new(icon).upcast()) }
    }

    fn mount(mount: &gio::Mount) -> Self {
        Self { file: mount.root(), name: mount.name().into(), icon: Some(mount.symbolic_icon()) }
    }
}

/// How many fixed rows sit above the mounted drives.
const FIXED_ROOTS: u32 = 2;

pub struct FolderTree {
    widget: gtk::ScrolledWindow,
    selection: gtk::SingleSelection,
    /// Kept alive so mount and unmount events keep arriving.
    _volumes: gio::VolumeMonitor,
}

impl FolderTree {
    pub fn new() -> Rc<Self> {
        let roots = gio::ListStore::new::<glib::BoxedAnyObject>();
        roots.append(&glib::BoxedAnyObject::new(Node::place(glib::home_dir(), "Home", "user-home-symbolic")));
        roots.append(&glib::BoxedAnyObject::new(Node::place("/", "Computer", "drive-harddisk-symbolic")));

        // Mounted drives and network shares, kept current as they come and go.
        let volumes = gio::VolumeMonitor::get();
        let refresh_mounts = {
            let roots = roots.clone();
            move |volumes: &gio::VolumeMonitor| {
                let mut mounts: Vec<gio::Mount> = volumes.mounts().into_iter().filter(|m| !m.is_shadowed()).collect();
                mounts.sort_by_cached_key(|m| m.name().to_lowercase());
                let items: Vec<glib::BoxedAnyObject> = mounts.iter().map(|m| glib::BoxedAnyObject::new(Node::mount(m))).collect();
                roots.splice(FIXED_ROOTS, roots.n_items() - FIXED_ROOTS, &items);
            }
        };
        refresh_mounts(&volumes);
        {
            let refresh = refresh_mounts.clone();
            volumes.connect_mount_added(move |v, _| refresh(v));
        }
        volumes.connect_mount_removed(move |v, _| refresh_mounts(v));

        // Every folder gets an expander; its children are listed when opened.
        let tree = gtk::TreeListModel::new(roots, false, false, |item| {
            let node = item.downcast_ref::<glib::BoxedAnyObject>()?.borrow::<Node>().clone();
            let children = gio::ListStore::new::<glib::BoxedAnyObject>();
            fill_children(children.clone(), node.file);
            Some(children.upcast())
        });

        let selection = gtk::SingleSelection::builder().model(&tree).autoselect(false).build();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let icon = gtk::Image::from_icon_name("folder-symbolic");
            let label = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).xalign(0.0).build();
            let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).build();
            row.append(&icon);
            row.append(&label);
            let expander = gtk::TreeExpander::builder().child(&row).indent_for_icon(true).build();
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = item.item().and_downcast::<gtk::TreeListRow>().unwrap();
            let expander = item.child().and_downcast::<gtk::TreeExpander>().unwrap();
            expander.set_list_row(Some(&row));
            let node = row.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
            let row = expander.child().and_downcast::<gtk::Box>().unwrap();
            let icon = row.first_child().and_downcast::<gtk::Image>().unwrap();
            let label = row.last_child().and_downcast::<gtk::Label>().unwrap();
            let node = node.borrow::<Node>();
            match &node.icon {
                Some(gicon) => icon.set_from_gicon(gicon),
                None => icon.set_icon_name(Some("folder-symbolic")),
            }
            label.set_text(&node.name);
        });

        let view = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .css_classes(["navigation-sidebar"])
            .build();

        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .width_request(240)
            .build();

        Rc::new(Self { widget, selection, _volumes: volumes })
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.widget
    }

    /// Called with the folder whenever the user picks a row.
    pub fn connect_folder_selected(&self, f: impl Fn(gio::File) + 'static) {
        self.selection.connect_selected_item_notify(move |selection| {
            let Some(row) = selection.selected_item().and_downcast::<gtk::TreeListRow>() else { return };
            let Some(node) = row.item().and_downcast::<glib::BoxedAnyObject>() else { return };
            f(node.borrow::<Node>().file.clone());
        });
    }
}

/// List the visible subfolders of `dir` into `store`, sorted by name,
/// without blocking the window.
fn fill_children(store: gio::ListStore, dir: gio::File) {
    glib::spawn_future_local(async move {
        let attributes = [
            gio::FILE_ATTRIBUTE_STANDARD_NAME.as_str(),
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            gio::FILE_ATTRIBUTE_STANDARD_IS_HIDDEN.as_str(),
        ]
        .join(",");
        let Ok(enumerator) = dir
            .enumerate_children_future(&attributes, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT)
            .await
        else {
            return;
        };
        let mut folders = Vec::new();
        while let Ok(batch) = enumerator.next_files_future(256, glib::Priority::DEFAULT).await {
            if batch.is_empty() {
                break;
            }
            for info in batch {
                if info.file_type() == gio::FileType::Directory && !info.is_hidden() {
                    let name = info.name();
                    folders.push(Node::folder(dir.child(&name), name.to_string_lossy().into_owned()));
                }
            }
        }
        folders.sort_by_cached_key(|n| n.name.to_lowercase());
        let items: Vec<glib::BoxedAnyObject> = folders.into_iter().map(glib::BoxedAnyObject::new).collect();
        store.splice(0, 0, &items);
    });
}
