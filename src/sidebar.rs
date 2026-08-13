//! The Navigator (§6.6): a left sidebar of folders, files, and the headings
//! inside each file. `GtkListView` over a `TreeListModel` so it stays lazy —
//! folder children are separate `ListStore`s that fill as the scan streams
//! in, and a file's headings are parsed only when its row is expanded.
//! Expansion state is ephemeral, persisted nowhere.

use crate::headings;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Node {
    Folder { name: String, rel: PathBuf },
    File { name: String, abs: PathBuf },
    Heading { text: String, id: String, level: u8, file: PathBuf },
}

pub enum SidebarAction {
    OpenFile(PathBuf),
    ScrollTo { file: PathBuf, id: String },
}

struct Inner {
    root: PathBuf,
    // relative dir → its children store; "" is the tree root
    stores: RefCell<HashMap<PathBuf, gio::ListStore>>,
    // absolute file path → its (materialized) headings store, so live
    // reload can replace the rows in place
    heading_stores: RefCell<HashMap<PathBuf, gio::ListStore>>,
}

pub struct Sidebar {
    pub widget: gtk4::ScrolledWindow,
    inner: Rc<Inner>,
}

impl Sidebar {
    pub fn new(root_dir: &Path, on_activate: impl Fn(SidebarAction) + 'static) -> Sidebar {
        let inner = Rc::new(Inner {
            root: root_dir.to_path_buf(),
            stores: RefCell::new(HashMap::new()),
            heading_stores: RefCell::new(HashMap::new()),
        });

        let root_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        inner
            .stores
            .borrow_mut()
            .insert(PathBuf::new(), root_store.clone());

        let tree_model = {
            let inner = inner.clone();
            gtk4::TreeListModel::new(root_store, false, false, move |item| {
                children_for(&inner, item)
            })
        };
        let selection = gtk4::SingleSelection::new(Some(tree_model));

        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            let icon = gtk4::Image::new();
            let label = gtk4::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();
            let row_box = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Horizontal)
                .spacing(6)
                .build();
            row_box.append(&icon);
            row_box.append(&label);
            let expander = gtk4::TreeExpander::new();
            expander.set_child(Some(&row_box));
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            let row = item.item().and_downcast::<gtk4::TreeListRow>().unwrap();
            let expander = item.child().and_downcast::<gtk4::TreeExpander>().unwrap();
            expander.set_list_row(Some(&row));
            let row_box = expander.child().and_downcast::<gtk4::Box>().unwrap();
            let icon = row_box.first_child().and_downcast::<gtk4::Image>().unwrap();
            let label = row_box.last_child().and_downcast::<gtk4::Label>().unwrap();
            label.remove_css_class("moremaid-heading-row");

            let obj = row.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
            let node = obj.borrow::<Node>();
            match &*node {
                Node::Folder { name, .. } => {
                    icon.set_icon_name(Some("folder-symbolic"));
                    icon.set_visible(true);
                    label.set_text(name);
                    label.set_margin_start(0);
                }
                Node::File { name, .. } => {
                    icon.set_icon_name(Some("text-x-generic-symbolic"));
                    icon.set_visible(true);
                    label.set_text(name);
                    label.set_margin_start(0);
                }
                Node::Heading { text, level, .. } => {
                    icon.set_visible(false);
                    label.set_text(text);
                    label.set_margin_start(((*level as i32) - 1) * 10);
                    label.add_css_class("moremaid-heading-row");
                }
            }
        });

        let list_view = gtk4::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(true)
            .build();
        list_view.add_css_class("moremaid-sidebar");
        list_view.add_css_class("navigation-sidebar");

        {
            let selection = selection.clone();
            list_view.connect_activate(move |_, position| {
                let Some(row) = selection.item(position).and_downcast::<gtk4::TreeListRow>()
                else {
                    return;
                };
                let Some(obj) = row.item().and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                let node = obj.borrow::<Node>().clone();
                match node {
                    Node::Folder { .. } => row.set_expanded(!row.is_expanded()),
                    Node::File { abs, .. } => {
                        // a click both opens the file and reveals its headings
                        row.set_expanded(true);
                        on_activate(SidebarAction::OpenFile(abs));
                    }
                    Node::Heading { file, id, .. } => {
                        on_activate(SidebarAction::ScrollTo { file, id });
                    }
                }
            });
        }

        let widget = gtk4::ScrolledWindow::builder()
            .child(&list_view)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .width_request(240)
            .build();
        widget.add_css_class("moremaid-sidebar");

        Sidebar { widget, inner }
    }

    /// Replace a file's heading rows after a live reload changed them.
    /// A no-op until the file's headings were materialized by expansion.
    pub fn update_headings(&self, file: &Path, hs: Vec<headings::Heading>) {
        if let Some(store) = self.inner.heading_stores.borrow().get(file) {
            store.remove_all();
            fill_heading_store(store, file, hs);
        }
    }

    /// Feed a batch of markdown paths (absolute, under the root) into the
    /// tree. Called on the main loop as scan batches stream in.
    pub fn add_files(&self, paths: &[PathBuf]) {
        for abs in paths {
            let Ok(rel) = abs.strip_prefix(&self.inner.root) else {
                continue;
            };
            let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
            let store = ensure_dir_store(&self.inner, &dir);
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            insert_sorted(
                &store,
                Node::File {
                    name,
                    abs: abs.clone(),
                },
            );
        }
    }
}

/// The store for a relative directory, creating it — and its folder row in
/// the parent, recursively — on first sight.
fn ensure_dir_store(inner: &Rc<Inner>, rel_dir: &Path) -> gio::ListStore {
    if let Some(store) = inner.stores.borrow().get(rel_dir) {
        return store.clone();
    }
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    inner
        .stores
        .borrow_mut()
        .insert(rel_dir.to_path_buf(), store.clone());

    let parent = rel_dir.parent().unwrap_or(Path::new("")).to_path_buf();
    let parent_store = ensure_dir_store(inner, &parent);
    let name = rel_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    insert_sorted(
        &parent_store,
        Node::Folder {
            name,
            rel: rel_dir.to_path_buf(),
        },
    );
    store
}

/// Folders before files, then case-insensitive by name.
fn insert_sorted(store: &gio::ListStore, node: Node) {
    let obj = glib::BoxedAnyObject::new(node);
    store.insert_sorted(&obj, |a, b| {
        let a = a.downcast_ref::<glib::BoxedAnyObject>().unwrap();
        let b = b.downcast_ref::<glib::BoxedAnyObject>().unwrap();
        let a = a.borrow::<Node>();
        let b = b.borrow::<Node>();
        rank(&a)
            .cmp(&rank(&b))
            .then_with(|| sort_name(&a).cmp(&sort_name(&b)))
    });
}

fn fill_heading_store(store: &gio::ListStore, file: &Path, hs: Vec<headings::Heading>) {
    for h in hs {
        // document order, not sorted
        store.append(&glib::BoxedAnyObject::new(Node::Heading {
            text: h.text,
            id: h.id,
            level: h.level,
            file: file.to_path_buf(),
        }));
    }
}

fn rank(node: &Node) -> u8 {
    match node {
        Node::Folder { .. } => 0,
        Node::File { .. } => 1,
        Node::Heading { .. } => 2,
    }
}

fn sort_name(node: &Node) -> String {
    match node {
        Node::Folder { name, .. } | Node::File { name, .. } => name.to_lowercase(),
        Node::Heading { text, .. } => text.to_lowercase(),
    }
}

/// TreeListModel child factory: folders expand to their store, files expand
/// to their headings (parsed right here, lazily), headings are leaves.
fn children_for(inner: &Rc<Inner>, item: &glib::Object) -> Option<gio::ListModel> {
    let obj = item.downcast_ref::<glib::BoxedAnyObject>()?;
    let node = obj.borrow::<Node>().clone();
    match node {
        Node::Folder { rel, .. } => {
            Some(ensure_dir_store(inner, &rel).upcast())
        }
        Node::File { abs, .. } => {
            let content = std::fs::read_to_string(&abs).ok()?;
            let hs = headings::extract_headings(&content);
            if hs.is_empty() {
                return None;
            }
            let store = gio::ListStore::new::<glib::BoxedAnyObject>();
            fill_heading_store(&store, &abs, hs);
            inner
                .heading_stores
                .borrow_mut()
                .insert(abs, store.clone());
            Some(store.upcast())
        }
        Node::Heading { .. } => None,
    }
}
