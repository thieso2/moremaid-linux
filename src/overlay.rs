//! Quick Open + Find in Files (§1, §6.5): one overlay, two modes.
//! `Ctrl+P` / `/` opens in filename mode, `Ctrl+Shift+F` in content mode,
//! `Tab` switches between them. Results stream in as they're found.

use crate::search;
use gtk4::gdk as gdk4;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_FILENAME_RESULTS: usize = 100;
const MAX_CONTENT_RESULTS: usize = 300;
const CONTENT_DEBOUNCE_MS: u64 = 250;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Filename,
    Content,
}

#[derive(Clone)]
enum Row {
    File { abs: PathBuf, rel: String },
    Match { abs: PathBuf, rel: String, line_number: u64, line: String, span: (usize, usize) },
}

pub enum OverlayAction {
    Open(PathBuf),
    OpenWithSearch(PathBuf, String),
}

pub struct SearchOverlay {
    pub widget: gtk4::Box,
    entry: gtk4::Entry,
    mode_label: gtk4::Label,
    store: gio::ListStore,
    selection: gtk4::SingleSelection,
    mode: Cell<Mode>,
    generation: Arc<AtomicU64>,
    debounce: RefCell<Option<glib::SourceId>>,
    all_files: Rc<RefCell<Vec<PathBuf>>>,
    root: PathBuf,
}

impl SearchOverlay {
    pub fn new(
        root: &Path,
        all_files: Rc<RefCell<Vec<PathBuf>>>,
        accent: &str,
        on_action: impl Fn(OverlayAction) + 'static,
    ) -> Rc<SearchOverlay> {
        let entry = gtk4::Entry::builder()
            .placeholder_text("Type to search…")
            .build();
        let mode_label = gtk4::Label::builder()
            .label("Files")
            .xalign(0.0)
            .build();
        mode_label.add_css_class("moremaid-overlay-mode");

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk4::SingleSelection::new(Some(store.clone()));

        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            let title = gtk4::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::Middle)
                .build();
            let snippet = gtk4::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .use_markup(true)
                .build();
            snippet.add_css_class("moremaid-overlay-snippet");
            let vbox = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .spacing(2)
                .build();
            vbox.append(&title);
            vbox.append(&snippet);
            item.set_child(Some(&vbox));
        });
        {
            let accent = accent.to_string();
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
                let vbox = item.child().and_downcast::<gtk4::Box>().unwrap();
                let title = vbox.first_child().and_downcast::<gtk4::Label>().unwrap();
                let snippet = vbox.last_child().and_downcast::<gtk4::Label>().unwrap();
                let obj = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
                let row = obj.borrow::<Row>().clone();
                match row {
                    Row::File { rel, .. } => {
                        title.set_text(&rel);
                        snippet.set_visible(false);
                    }
                    Row::Match { rel, line_number, line, span, .. } => {
                        title.set_text(&format!("{rel}:{line_number}"));
                        snippet.set_markup(&highlight_markup(&line, span, &accent));
                        snippet.set_visible(true);
                    }
                }
            });
        }

        let list = gtk4::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(true)
            .build();
        let scroller = gtk4::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .max_content_height(360)
            .propagate_natural_height(true)
            .build();

        let header = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();
        header.append(&mode_label);
        header.append(&entry);
        entry.set_hexpand(true);

        let widget = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .width_request(640)
            .visible(false)
            .build();
        widget.add_css_class("moremaid-overlay");
        widget.append(&header);
        widget.append(&scroller);

        let overlay = Rc::new(SearchOverlay {
            widget,
            entry: entry.clone(),
            mode_label,
            store,
            selection: selection.clone(),
            mode: Cell::new(Mode::Filename),
            generation: Arc::new(AtomicU64::new(0)),
            debounce: RefCell::new(None),
            all_files,
            root: root.to_path_buf(),
        });

        let on_action = Rc::new(on_action);

        {
            let ov = overlay.clone();
            entry.connect_changed(move |_| ov.query_changed());
        }
        {
            let ov = overlay.clone();
            let on_action = on_action.clone();
            entry.connect_activate(move |_| ov.activate_selected(&on_action));
        }
        {
            let ov = overlay.clone();
            let on_action = on_action.clone();
            list.connect_activate(move |_, position| {
                ov.selection.set_selected(position);
                ov.activate_selected(&on_action);
            });
        }

        // Escape closes, Tab switches mode, Up/Down move the selection while
        // the entry keeps focus (§6.5).
        {
            let ov = overlay.clone();
            let keys = gtk4::EventControllerKey::new();
            keys.connect_key_pressed(move |_, keyval, _, _| {
                match keyval {
                    gdk4::Key::Escape => {
                        ov.close();
                        glib::Propagation::Stop
                    }
                    gdk4::Key::Tab => {
                        ov.toggle_mode();
                        glib::Propagation::Stop
                    }
                    gdk4::Key::Down => {
                        ov.move_selection(1);
                        glib::Propagation::Stop
                    }
                    gdk4::Key::Up => {
                        ov.move_selection(-1);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            entry.add_controller(keys);
        }

        overlay
    }

    pub fn open(&self, mode: Mode) {
        self.mode.set(mode);
        self.mode_label.set_text(match mode {
            Mode::Filename => "Files",
            Mode::Content => "Text",
        });
        self.widget.set_visible(true);
        self.entry.grab_focus();
        self.query_changed();
    }

    pub fn close(&self) {
        // cancel any in-flight content search
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.widget.set_visible(false);
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    fn toggle_mode(&self) {
        let next = match self.mode.get() {
            Mode::Filename => Mode::Content,
            Mode::Content => Mode::Filename,
        };
        self.open(next);
    }

    fn move_selection(&self, delta: i64) {
        let n = self.selection.n_items() as i64;
        if n == 0 {
            return;
        }
        let current = self.selection.selected() as i64;
        let next = (current + delta).clamp(0, n - 1);
        self.selection.set_selected(next as u32);
    }

    fn activate_selected(&self, on_action: &Rc<impl Fn(OverlayAction) + 'static>) {
        let Some(obj) = self
            .selection
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()
        else {
            return;
        };
        let row = obj.borrow::<Row>().clone();
        let query = self.entry.text().to_string();
        self.close();
        match row {
            Row::File { abs, .. } => on_action(OverlayAction::Open(abs)),
            Row::Match { abs, .. } => on_action(OverlayAction::OpenWithSearch(abs, query)),
        }
    }

    fn query_changed(&self) {
        // a newer query supersedes any running search
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(source) = self.debounce.borrow_mut().take() {
            source.remove();
        }
        match self.mode.get() {
            Mode::Filename => self.run_filename_query(),
            Mode::Content => self.schedule_content_query(),
        }
    }

    fn rel_of(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.root)
            .unwrap_or(abs)
            .to_string_lossy()
            .to_string()
    }

    fn run_filename_query(&self) {
        let query = self.entry.text().to_string();
        let files = self.all_files.borrow();
        let rels: Vec<String> = files.iter().map(|p| self.rel_of(p)).collect();
        let ranked = search::fuzzy_rank(&rels, &query);
        self.store.remove_all();
        for &i in ranked.iter().take(MAX_FILENAME_RESULTS) {
            self.store.append(&glib::BoxedAnyObject::new(Row::File {
                abs: files[i].clone(),
                rel: rels[i].clone(),
            }));
        }
        if self.selection.n_items() > 0 {
            self.selection.set_selected(0);
        }
    }

    fn schedule_content_query(&self) {
        let query = self.entry.text().to_string();
        self.store.remove_all();
        if query.len() < 2 {
            return;
        }
        let my_generation = self.generation.load(Ordering::SeqCst);
        let generation = self.generation.clone();
        let files: Vec<PathBuf> = self.all_files.borrow().clone();
        let store = self.store.clone();
        let selection = self.selection.clone();
        let root = self.root.clone();

        let source = glib::timeout_add_local_once(
            std::time::Duration::from_millis(CONTENT_DEBOUNCE_MS),
            move || {
                if generation.load(Ordering::SeqCst) != my_generation {
                    return;
                }
                let (tx, rx) = async_channel::bounded::<search::SearchMatch>(64);
                {
                    let generation = generation.clone();
                    let query = query.clone();
                    std::thread::spawn(move || {
                        let mut n = 0usize;
                        search::search_into(&files, &query, &generation, my_generation, |m| {
                            n += 1;
                            tx.send_blocking(m).is_ok() && n < MAX_CONTENT_RESULTS
                        });
                    });
                }
                glib::spawn_future_local(async move {
                    let mut first = true;
                    while let Ok(m) = rx.recv().await {
                        if generation.load(Ordering::SeqCst) != my_generation {
                            break;
                        }
                        let rel = m
                            .path
                            .strip_prefix(&root)
                            .unwrap_or(&m.path)
                            .to_string_lossy()
                            .to_string();
                        store.append(&glib::BoxedAnyObject::new(Row::Match {
                            abs: m.path,
                            rel,
                            line_number: m.line_number,
                            line: m.line,
                            span: m.span,
                        }));
                        if first {
                            selection.set_selected(0);
                            first = false;
                        }
                    }
                });
            },
        );
        self.debounce.replace(Some(source));
    }
}

/// Pango markup for a match line, the matched span in the accent colour.
fn highlight_markup(line: &str, span: (usize, usize), accent: &str) -> String {
    let (start, end) = span;
    if start >= end || end > line.len() || !line.is_char_boundary(start) || !line.is_char_boundary(end)
    {
        return glib::markup_escape_text(line).to_string();
    }
    format!(
        "{}<span foreground=\"{}\" weight=\"bold\">{}</span>{}",
        glib::markup_escape_text(&line[..start]),
        accent,
        glib::markup_escape_text(&line[start..end]),
        glib::markup_escape_text(&line[end..]),
    )
}


