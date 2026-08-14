//! Moremaid — a markdown reader for Omarchy/Hyprland.
//!
//! One window per invocation, no in-app tabs, no session restore (§6.1).
//! The compositor owns window management; the app renders documents.

mod config;
mod headings;
mod html;
mod langmap;
mod overlay;
mod scan;
mod search;
mod sidebar;
mod theme;
mod watch;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use webkit6::prelude::*;

const APP_ID: &str = "org.moremaid.Moremaid";

enum Target {
    File(PathBuf),
    Dir(PathBuf),
    Stdin(String),
}

#[derive(Clone, PartialEq)]
enum HistoryEntry {
    File(PathBuf),
    Dir(PathBuf),
}

struct TopBar {
    widget: gtk4::Box,
    back: gtk4::Button,
    forward: gtk4::Button,
    title: gtk4::Label,
}

fn main() -> glib::ExitCode {
    let started = Instant::now();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = match parse_target(&args) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("moremaid: {msg}");
            return glib::ExitCode::from(1);
        }
    };

    let Some(web_dir) = html::web_dir() else {
        eprintln!("moremaid: web assets not found (looked in $MOREMAID_WEB_DIR, ~/.local/share/moremaid/web, /usr/share/moremaid/web)");
        return glib::ExitCode::from(1);
    };

    let app = gtk4::Application::builder()
        .application_id(APP_ID)
        // Without NON_UNIQUE a second invocation forwards its arguments to the
        // first process over D-Bus and silently exits (§6.1).
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let target = RefCell::new(Some(target));
    app.connect_activate(move |app| {
        if let Some(target) = target.borrow_mut().take() {
            build_window(app, target, web_dir.clone(), started);
        }
    });

    // Arguments were parsed by hand above; don't let GApplication see them.
    app.run_with_args::<&str>(&[])
}

fn parse_target(args: &[String]) -> Result<Target, String> {
    match args.first() {
        Some(arg) => {
            let path = PathBuf::from(arg);
            if !path.exists() {
                return Err(format!("{arg}: no such file or directory"));
            }
            let path = path
                .canonicalize()
                .map_err(|e| format!("{arg}: {e}"))?;
            if path.is_dir() {
                return Ok(Target::Dir(path));
            }
            let data = std::fs::read(&path).map_err(|e| format!("{arg}: {e}"))?;
            if data[..data.len().min(8192)].contains(&0) {
                return Err(format!(
                    "{} is a binary file ({} bytes) — Moremaid reads text documents",
                    path.display(),
                    data.len()
                ));
            }
            Ok(Target::File(path))
        }
        None => {
            if std::io::stdin().is_terminal() {
                // bare `moremaid` browses the current directory (§10)
                std::env::current_dir()
                    .map_err(|e| format!("current directory: {e}"))
                    .map(Target::Dir)
            } else {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| format!("stdin: {e}"))?;
                Ok(Target::Stdin(buf))
            }
        }
    }
}

struct WindowCtx {
    webview: webkit6::WebView,
    window: gtk4::ApplicationWindow,
    palette: RefCell<theme::Palette>,
    fonts: config::Fonts,
    web_dir: PathBuf,
    doc_path: RefCell<Option<PathBuf>>,
    stdin_src: RefCell<Option<String>>,
    base_uri: RefCell<String>,
    force_full: Cell<bool>,
    /// anchor to jump to once the pending document finishes loading
    pending_anchor: RefCell<Option<String>>,
    /// search query to highlight once the pending document finishes loading
    pending_search: RefCell<Option<String>>,
    sidebar: RefCell<Option<Rc<sidebar::Sidebar>>>,
    overlay: RefCell<Option<Rc<overlay::SearchOverlay>>>,
    /// browse root in directory mode; drives Ctrl+N and Quick Open
    root_dir: RefCell<Option<PathBuf>>,
    all_files: Rc<RefCell<Vec<PathBuf>>>,
    /// watches the open document's parent directory (§9.4); dropping it
    /// stops the previous watch when the document changes
    doc_watcher: RefCell<Option<watch::DirWatcher>>,
    /// watches ~/.local/state/omarchy/current/ for theme switches
    theme_watcher: RefCell<Option<watch::DirWatcher>>,
    /// the app's own zoom factor; effective content zoom is this times the
    /// system text-scaling-factor (§6.4)
    app_zoom: Cell<f64>,
    text_scale: Cell<f64>,
    /// kept alive for the text-scaling-factor changed signal
    interface_settings: RefCell<Option<gio::Settings>>,
    /// Ctrl+B pins the Navigator open; unpinned it auto-hides and reveals
    /// on a left-edge hover
    sidebar_pinned: Cell<bool>,
    /// navigation history — load_html has no WebKit history, so back and
    /// forward are ours to keep
    history: RefCell<Vec<HistoryEntry>>,
    history_pos: Cell<usize>,
    /// true while executing a back/forward jump, so the resulting load
    /// doesn't re-record itself
    in_history_nav: Cell<bool>,
    topbar: RefCell<Option<Rc<TopBar>>>,
    /// Ctrl+M: show the raw markdown as a highlighted code page instead of
    /// rendering it
    view_source: Cell<bool>,
    /// monotonically increasing per-load token, echoed back by the page's
    /// loadComplete so a superseded page cannot consume the next page's
    /// pending anchor or search highlight
    load_seq: Cell<u64>,
    /// true once the CURRENT load's loadComplete arrived
    load_complete: Cell<bool>,
}

fn trace(started: Instant, what: &str) {
    if std::env::var_os("MOREMAID_STARTUP_TIME").is_some() {
        eprintln!("moremaid: +{:>5} ms  {what}", started.elapsed().as_millis());
    }
}

fn build_window(app: &gtk4::Application, target: Target, web_dir: PathBuf, started: Instant) {
    trace(started, "activate");
    // Palette is read synchronously before the first load_html so there is
    // never a flash of the wrong colours (§6.3, §9.5).
    let palette = theme::Palette::load(adw_dark_hint);
    let fonts = config::load().fonts();
    trace(started, "palette loaded");

    ensure_uri_scheme(&web_dir);

    let ucm = webkit6::UserContentManager::new();
    ucm.register_script_message_handler("moremaid", None);
    ucm.register_script_message_handler("openDiagram", None);

    let webview = webkit6::WebView::builder()
        .user_content_manager(&ucm)
        .build();

    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(true);
        if std::env::var_os("MOREMAID_DEBUG").is_some() {
            settings.set_enable_write_console_messages_to_stdout(true);
        }
    }

    // Paint the palette background before the document arrives — no flash.
    if let Ok(bg) = palette.get("background").parse::<gdk::RGBA>() {
        webview.set_background_color(&bg);
    }

    // Bare window: Hyprland answers server-side decorations, GTK suppresses
    // its fallback titlebar, and a header bar would be chrome for content
    // that doesn't exist (§6.2).
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(1100)
        .default_height(800)
        .build();

    let ctx = Rc::new(WindowCtx {
        webview: webview.clone(),
        window: window.clone(),
        palette: RefCell::new(palette),
        fonts,
        web_dir,
        doc_path: RefCell::new(None),
        stdin_src: RefCell::new(None),
        base_uri: RefCell::new(String::new()),
        force_full: Cell::new(false),
        pending_anchor: RefCell::new(None),
        pending_search: RefCell::new(None),
        sidebar: RefCell::new(None),
        overlay: RefCell::new(None),
        root_dir: RefCell::new(None),
        all_files: Rc::new(RefCell::new(Vec::new())),
        doc_watcher: RefCell::new(None),
        theme_watcher: RefCell::new(None),
        app_zoom: Cell::new(1.0),
        text_scale: Cell::new(1.0),
        interface_settings: RefCell::new(None),
        sidebar_pinned: Cell::new(false),
        history: RefCell::new(Vec::new()),
        history_pos: Cell::new(0),
        in_history_nav: Cell::new(false),
        topbar: RefCell::new(None),
        view_source: Cell::new(false),
        load_seq: Cell::new(0),
        load_complete: Cell::new(false),
    });
    watch_text_scaling(&ctx);

    // JS → Rust bridge. Payloads are JSON strings (or, for openDiagram, the
    // raw diagram definition) — see web/js/page.js.
    {
        let ctx = ctx.clone();
        ucm.connect_script_message_received(Some("moremaid"), move |_, value| {
            let Some(s) = value.to_str().into() else { return };
            let s: String = s.to_string();
            if std::env::var_os("MOREMAID_STARTUP_TIME").is_some() {
                if s.starts_with("{\"type\":\"firstRender\"") {
                    eprintln!(
                        "moremaid: cold start → painted document in {} ms",
                        started.elapsed().as_millis()
                    );
                } else if s.starts_with("{\"type\":\"loadComplete\"") {
                    eprintln!(
                        "moremaid: cold start → loadComplete in {} ms",
                        started.elapsed().as_millis()
                    );
                }
            }
            if s.starts_with("{\"type\":\"loadComplete\"") {
                // only the CURRENT load's completion may consume pending
                // work — a superseded page's message must not (token check)
                let token = serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .and_then(|v| v.get("token")?.as_u64());
                if token == Some(ctx.load_seq.get()) {
                    ctx.load_complete.set(true);
                    if let Some(anchor) = ctx.pending_anchor.borrow_mut().take() {
                        scroll_to_anchor(&ctx, &anchor);
                    }
                    if let Some(query) = ctx.pending_search.borrow_mut().take() {
                        let js = format!("highlightSearchQuery({});", html::json_str(&query));
                        ctx.webview
                            .evaluate_javascript(&js, None, None, None::<&gio::Cancellable>, |_| {});
                    }
                }
            }
            if s.starts_with("{\"type\":\"headings\"") {
                // live reload can change the heading set — keep the
                // Navigator's rows for the open file in sync
                let doc = ctx.doc_path.borrow().clone();
                let sb = ctx.sidebar.borrow().clone();
                if let (Some(doc), Some(sb)) = (doc, sb) {
                    if let Some(hs) = parse_headings_message(&s) {
                        sb.update_headings(&doc, hs);
                    }
                }
            }
        });
    }
    {
        let ctx = ctx.clone();
        let app = app.clone();
        ucm.connect_script_message_received(Some("openDiagram"), move |_, value| {
            let definition = value.to_str().to_string();
            open_diagram_window(&app, &ctx, &definition);
        });
    }

    connect_link_policy(&ctx, app);
    install_actions(app, &ctx);

    {
        let ctx = ctx.clone();
        webview.connect_load_changed(move |_, event| {
            if event == webkit6::LoadEvent::Finished {
                trace(started, "webkit load finished");
                // raw HTML documents carry no moremaid bridge, so their
                // pending anchor resolves here instead of on loadComplete
                let is_html_doc = ctx
                    .doc_path
                    .borrow()
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .map(|n| langmap::is_html(&n.to_string_lossy()))
                    .unwrap_or(false);
                if is_html_doc && !ctx.view_source.get() {
                    ctx.load_complete.set(true);
                    if let Some(anchor) = ctx.pending_anchor.borrow_mut().take() {
                        scroll_to_anchor(&ctx, &anchor);
                    }
                }
            }
        });
    }

    arm_theme_watcher(&ctx);

    // Every mode shares the overlay stack: webview at the bottom, then the
    // Navigator (dir mode), the top bar, and the search overlay on top.
    apply_chrome_css(&ctx.palette.borrow(), &ctx.fonts);
    let overlay_root = gtk4::Overlay::new();
    overlay_root.set_child(Some(&webview));
    let topbar = build_topbar(&ctx);
    ctx.topbar.replace(Some(topbar.clone()));

    match target {
        Target::File(path) => {
            overlay_root.add_overlay(&topbar.widget);
            window.set_child(Some(&overlay_root));
            install_edge_reveal(&ctx, &overlay_root);
            load_path(&ctx, &path);
        }
        Target::Stdin(src) => {
            overlay_root.add_overlay(&topbar.widget);
            window.set_child(Some(&overlay_root));
            install_edge_reveal(&ctx, &overlay_root);
            load_stdin(&ctx, src);
        }
        Target::Dir(dir) => {
            ctx.root_dir.replace(Some(dir.clone()));
            let sb = {
                let ctx = ctx.clone();
                Rc::new(sidebar::Sidebar::new(&dir, move |action| match action {
                    sidebar::SidebarAction::OpenFile(path) => {
                        load_path(&ctx, &path);
                    }
                    sidebar::SidebarAction::ScrollTo { file, id } => {
                        if ctx.doc_path.borrow().as_deref() == Some(file.as_path())
                            && ctx.load_complete.get()
                        {
                            scroll_to_anchor(&ctx, &id);
                        } else {
                            // the doc may already be the target but still
                            // loading — then the pending path applies too
                            let same = ctx.doc_path.borrow().as_deref() == Some(file.as_path());
                            if same || load_path(&ctx, &file) {
                                // set only after the load was issued — a
                                // failed load must not leave a stale anchor
                                ctx.pending_anchor.replace(Some(id));
                            }
                        }
                    }
                }))
            };
            // The Navigator auto-hides: it overlays the content, revealed by
            // a left-edge hover or pinned open with Ctrl+B — reading gets
            // the full width by default.
            sb.widget.set_halign(gtk4::Align::Start);
            sb.widget.set_valign(gtk4::Align::Fill);
            sb.widget.set_width_request(280);
            sb.widget.set_visible(false);

            // Quick Open / Find in Files float above the content (§6.5).
            let search_overlay = {
                let ctx = ctx.clone();
                let accent = ctx.palette.borrow().get("accent").to_string();
                let all_files = ctx.all_files.clone();
                overlay::SearchOverlay::new(
                    &dir,
                    all_files,
                    &accent,
                    move |action| match action {
                        overlay::OverlayAction::Open(path) => {
                            load_path(&ctx, &path);
                        }
                        overlay::OverlayAction::OpenWithSearch(path, query) => {
                            if ctx.doc_path.borrow().as_deref() == Some(path.as_path())
                                && ctx.load_complete.get()
                            {
                                let js = format!(
                                    "highlightSearchQuery({});",
                                    html::json_str(&query)
                                );
                                ctx.webview.evaluate_javascript(
                                    &js,
                                    None,
                                    None,
                                    None::<&gio::Cancellable>,
                                    |_| {},
                                );
                            } else {
                                let same =
                                    ctx.doc_path.borrow().as_deref() == Some(path.as_path());
                                if same || load_path(&ctx, &path) {
                                    ctx.pending_search.replace(Some(query));
                                }
                            }
                        }
                    },
                )
            };
            {
                // focus returns to the document whenever the overlay closes
                let webview = webview.clone();
                search_overlay.widget.connect_visible_notify(move |w| {
                    if !w.is_visible() {
                        webview.grab_focus();
                    }
                });
            }
            overlay_root.add_overlay(&sb.widget);
            overlay_root.add_overlay(&topbar.widget);
            search_overlay.widget.set_halign(gtk4::Align::Center);
            search_overlay.widget.set_valign(gtk4::Align::Start);
            search_overlay.widget.set_margin_top(48);
            overlay_root.add_overlay(&search_overlay.widget);
            window.set_child(Some(&overlay_root));
            ctx.sidebar.replace(Some(sb.clone()));
            ctx.overlay.replace(Some(search_overlay));
            install_edge_reveal(&ctx, &overlay_root);

            load_auto_index(&ctx, &dir);

            // Stream the scan into the Navigator (§9.1: first rows early).
            let rx = scan::scan_markdown(&dir);
            let all_files = ctx.all_files.clone();
            glib::spawn_future_local(async move {
                let mut first = true;
                let mut total = 0usize;
                while let Ok(batch) = rx.recv().await {
                    total += batch.len();
                    all_files.borrow_mut().extend(batch.iter().cloned());
                    sb.add_files(&batch);
                    if first {
                        trace(started, "scan first rows");
                        first = false;
                    }
                }
                trace(started, &format!("scan complete ({total} markdown files)"));
            });
        }
    }
    install_vim_keys(&ctx);
    install_drop_target(&ctx);
    trace(started, "load_html issued");

    window.present();
}

/// Bare-key bindings (§6.5): vim keys are baseline, not a power-user
/// affordance. Captured at the window so they work while the webview has
/// focus, and stand down whenever the search overlay is open (typing).
fn install_vim_keys(ctx: &Rc<WindowCtx>) {
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let ctx_keys = ctx.clone();
    let pending_g = Rc::new(Cell::new(false));
    keys.connect_key_pressed(move |_, keyval, _, state| {
        let ctx = &ctx_keys;
        if ctx.overlay.borrow().as_ref().is_some_and(|o| o.is_open()) {
            return glib::Propagation::Proceed;
        }
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let scroll = |js: &str| {
            ctx.webview
                .evaluate_javascript(js, None, None, None::<&gio::Cancellable>, |_| {});
            glib::Propagation::Stop
        };
        let g_was_pending = pending_g.replace(false);
        match keyval {
            gdk::Key::j if !ctrl => scroll("window.scrollBy(0, 80);"),
            gdk::Key::k if !ctrl => scroll("window.scrollBy(0, -80);"),
            gdk::Key::d if ctrl => scroll("window.scrollBy(0, window.innerHeight / 2);"),
            gdk::Key::u if ctrl => scroll("window.scrollBy(0, -window.innerHeight / 2);"),
            gdk::Key::G => scroll("window.scrollTo(0, document.body.scrollHeight);"),
            gdk::Key::g if !ctrl => {
                if g_was_pending {
                    return scroll("window.scrollTo(0, 0);");
                }
                pending_g.set(true);
                glib::Propagation::Stop
            }
            gdk::Key::slash => {
                if let Some(overlay) = &*ctx.overlay.borrow() {
                    overlay.open(overlay::Mode::Filename);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            gdk::Key::question => {
                show_shortcuts_dialog(&ctx.window);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    ctx.window.add_controller(keys);
}

/// The `?` overlay is the entire discoverability story (§6.5) — it replaces
/// the menu, so it lists everything.
fn show_shortcuts_dialog(window: &gtk4::ApplicationWindow) {
    if !libadwaita::is_initialized() && libadwaita::init().is_err() {
        return;
    }
    let dialog = libadwaita::ShortcutsDialog::new();
    let add_section = |title: &str, items: &[(&str, &str)]| {
        let section = libadwaita::ShortcutsSection::new(Some(title));
        for (name, accel) in items {
            section.add(libadwaita::ShortcutsItem::new(name, accel));
        }
        section
    };
    dialog.add(add_section(
        "Finding",
        &[
            ("Quick Open", "<Control>p"),
            ("Find in Files", "<Control><Shift>f"),
            ("Focus search", "slash"),
            ("Switch search mode", "Tab"),
            ("Open result", "Return"),
            ("Dismiss", "Escape"),
        ],
    ));
    dialog.add(add_section(
        "Reading",
        &[
            ("Scroll down / up", "j k"),
            ("Half page down / up", "<Control>d <Control>u"),
            ("Top of document", "g g"),
            ("End of document", "<Shift>g"),
            ("Zoom in / out", "<Control>plus <Control>minus"),
            ("Reset zoom", "<Control>0"),
            ("Rendered / raw markdown", "<Control>m"),
            ("Force full render", "<Control><Shift>r"),
        ],
    ));
    dialog.add(add_section(
        "Windows",
        &[
            ("Back / Forward", "<Alt>Left <Alt>Right"),
            ("Pin Navigator", "<Control>b"),
            ("New window", "<Control>n"),
            ("Open link in new window", "<Control>Pointer_Button1"),
            ("Shortcuts", "question"),
        ],
    ));
    libadwaita::prelude::AdwDialogExt::present(&dialog, Some(window));
}

/// Live reload (§9.4, M4): watch the open document's parent directory and
/// push changes through reRenderMarkdown/reRenderCode — never load_html, so
/// scroll position and the diagram cache survive. Deleted or replaced file →
/// keep the last good render behind a dismissible banner; reload and clear
/// if the path returns (§8 — the usual cause is a branch switch).
fn arm_doc_watcher(ctx: &Rc<WindowCtx>) {
    let Some(path) = ctx.doc_path.borrow().clone() else {
        return;
    };
    let Some(dir) = path.parent() else { return };
    let Some((debouncer, rx)) = watch::watch_dir(dir) else {
        return;
    };
    // dropping the previous debouncer closes its channel, ending its loop
    ctx.doc_watcher.replace(Some(debouncer));
    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        while let Ok(paths) = rx.recv().await {
            if ctx.doc_path.borrow().as_deref() != Some(path.as_path()) {
                break;
            }
            if !paths.iter().any(|p| p == &path) {
                continue;
            }
            refresh_doc(&ctx, &path);
        }
    });
}

fn refresh_doc(ctx: &Rc<WindowCtx>, path: &Path) {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // a rendered HTML document has no re-render entry points — reload it
    if langmap::is_html(&file_name) && !ctx.view_source.get() && path.exists() {
        load_path(ctx, path);
        return;
    }
    let js = match std::fs::read_to_string(path) {
        Ok(content) => {
            if langmap::is_markdown(&file_name) && !ctx.view_source.get() {
                format!(
                    "moremaidClearBanner(); reRenderMarkdown({});",
                    html::json_str(&content)
                )
            } else {
                format!(
                    "moremaidClearBanner(); reRenderCode({}, {});",
                    html::json_str(&content),
                    html::json_str(langmap::language_for_file(&file_name)),
                )
            }
        }
        Err(_) => format!(
            "moremaidShowBanner({});",
            html::json_str(&format!(
                "{file_name} is gone — keeping the last good render. It will reload if the file returns."
            )),
        ),
    };
    ctx.webview
        .evaluate_javascript(&js, None, None, None::<&gio::Cancellable>, |_| {});
}

/// Theme switches (§6.3): light↔dark arrives via StyleManager only off
/// Omarchy; the common case — Tokyo Night → Nord, same mode — signals
/// nothing, so watch ~/.local/state/omarchy/current/ (the PARENT directory;
/// the theme dir is replaced wholesale and a watch on it goes stale).
fn arm_theme_watcher(ctx: &Rc<WindowCtx>) {
    let home = std::env::var("HOME").unwrap_or_default();
    let current = PathBuf::from(&home).join(".local/state/omarchy/current");
    if current.is_dir() {
        if let Some((debouncer, rx)) = watch::watch_dir(&current) {
            ctx.theme_watcher.replace(Some(debouncer));
            let ctx = ctx.clone();
            glib::spawn_future_local(async move {
                while rx.recv().await.is_ok() {
                    retheme(&ctx);
                }
            });
        }
    } else if !ctx.palette.borrow().from_file && libadwaita::is_initialized() {
        // off Omarchy the light/dark bit is all there is
        let ctx = ctx.clone();
        libadwaita::StyleManager::default().connect_dark_notify(move |_| retheme(&ctx));
    }
}

/// Re-derive and push through evaluate_javascript, updating :root custom
/// properties in place. Never reload — rethemeing is exactly when the user
/// is mid-document (§6.3).
fn retheme(ctx: &Rc<WindowCtx>) {
    let new = theme::Palette::load(adw_dark_hint);
    if let Ok(bg) = new.get("background").parse::<gdk::RGBA>() {
        ctx.webview.set_background_color(&bg);
    }
    apply_chrome_css(&new, &ctx.fonts);
    let js = format!(
        "applyPalette({}, {}, {});",
        new.css_vars_json(&ctx.fonts),
        new.mermaid_vars_json(&ctx.fonts),
        html::json_str(if new.dark { "dark" } else { "light" }),
    );
    ctx.webview
        .evaluate_javascript(&js, None, None, None::<&gio::Cancellable>, |_| {});
    *ctx.palette.borrow_mut() = new;
}

/// GTK chrome honours text-scaling-factor automatically; WebKit content
/// does not (§6.4). effective content zoom = app zoom × text-scaling-factor,
/// live-updated when `omarchy display text size` changes the gsettings key.
fn watch_text_scaling(ctx: &Rc<WindowCtx>) {
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return;
    };
    if source.lookup("org.gnome.desktop.interface", true).is_none() {
        return;
    }
    let settings = gio::Settings::new("org.gnome.desktop.interface");
    ctx.text_scale.set(settings.double("text-scaling-factor"));
    apply_zoom(ctx);
    {
        let ctx = ctx.clone();
        settings.connect_changed(Some("text-scaling-factor"), move |s, _| {
            ctx.text_scale.set(s.double("text-scaling-factor"));
            apply_zoom(&ctx);
        });
    }
    ctx.interface_settings.replace(Some(settings));
}

fn apply_zoom(ctx: &Rc<WindowCtx>) {
    let scale = ctx.text_scale.get();
    let scale = if scale.is_finite() && scale > 0.1 { scale } else { 1.0 };
    ctx.webview.set_zoom_level(ctx.app_zoom.get() * scale);
}

/// Drag a file onto the window to read it (§1 "Fitting in").
fn install_drop_target(ctx: &Rc<WindowCtx>) {
    let drop = gtk4::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
    let ctx_drop = ctx.clone();
    drop.connect_drop(move |_, value, _, _| {
        let ctx = &ctx_drop;
        let Ok(file) = value.get::<gio::File>() else {
            return false;
        };
        let Some(path) = file.path() else { return false };
        if path.is_dir() {
            spawn_window_for(&path);
            true
        } else if is_text_file(&path) {
            load_path(ctx, &path)
        } else {
            false
        }
    });
    ctx.window.add_controller(drop);
}

/// One capture-phase motion controller drives both auto-hides: the top bar
/// reveals on a top-edge hover, the (unpinned) Navigator on a left-edge
/// hover; each retracts when the pointer moves past it.
fn install_edge_reveal(ctx: &Rc<WindowCtx>, overlay_root: &gtk4::Overlay) {
    let ctx = ctx.clone();
    let motion = gtk4::EventControllerMotion::new();
    // capture phase: the WebView must not swallow pointer motion
    motion.set_propagation_phase(gtk4::PropagationPhase::Capture);
    motion.connect_motion(move |_, x, y| {
        if let Some(topbar) = &*ctx.topbar.borrow() {
            let bar = &topbar.widget;
            if y <= 2.0 && !bar.is_visible() {
                bar.set_visible(true);
            } else if bar.is_visible() && y > f64::from(bar.height().max(40)) + 8.0 {
                bar.set_visible(false);
            }
        }
        if let Some(sb) = &*ctx.sidebar.borrow() {
            if ctx.sidebar_pinned.get() {
                return;
            }
            if x <= 2.0 && !sb.widget.is_visible() {
                sb.widget.set_visible(true);
            } else if sb.widget.is_visible()
                && x > f64::from(sb.widget.width().max(280)) + 8.0
            {
                sb.widget.set_visible(false);
            }
        }
    });
    overlay_root.add_controller(motion);
}

/// Record a completed navigation. A back/forward jump re-loads without
/// re-recording; a fresh navigation truncates the forward branch.
fn record_history(ctx: &Rc<WindowCtx>, entry: HistoryEntry) {
    if !ctx.in_history_nav.get() {
        let mut history = ctx.history.borrow_mut();
        let pos = ctx.history_pos.get();
        let is_current = history.get(pos) == Some(&entry);
        if !is_current {
            if !history.is_empty() {
                history.truncate(pos + 1);
            }
            history.push(entry);
            ctx.history_pos.set(history.len() - 1);
        }
    }
    update_nav_state(ctx);
}

fn go_history(ctx: &Rc<WindowCtx>, delta: i64) {
    let entry = {
        let history = ctx.history.borrow();
        let pos = ctx.history_pos.get() as i64 + delta;
        if pos < 0 || pos as usize >= history.len() {
            return;
        }
        ctx.history_pos.set(pos as usize);
        history[pos as usize].clone()
    };
    ctx.in_history_nav.set(true);
    match &entry {
        HistoryEntry::File(path) => {
            load_path(ctx, path);
        }
        HistoryEntry::Dir(dir) => load_auto_index(ctx, dir),
    }
    ctx.in_history_nav.set(false);
    update_nav_state(ctx);
}

fn update_nav_state(ctx: &Rc<WindowCtx>) {
    if let Some(topbar) = &*ctx.topbar.borrow() {
        let len = ctx.history.borrow().len();
        let pos = ctx.history_pos.get();
        topbar.back.set_sensitive(pos > 0 && len > 0);
        topbar.forward.set_sensitive(len > 0 && pos + 1 < len);
        let title = match ctx.history.borrow().get(pos) {
            Some(HistoryEntry::File(p)) | Some(HistoryEntry::Dir(p)) => {
                display_path(ctx, p)
            }
            None => String::new(),
        };
        topbar.title.set_text(&title);
    }
}

/// Breadcrumb-ish: relative to the browse root where there is one.
fn display_path(ctx: &Rc<WindowCtx>, path: &Path) -> String {
    if let Some(root) = &*ctx.root_dir.borrow() {
        if path == root {
            return root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string());
        }
        if let Ok(rel) = path.strip_prefix(root) {
            return rel.display().to_string();
        }
    }
    path.display().to_string()
}

/// Back/forward in an auto-hiding top bar — revealed by a top-edge hover,
/// like the Navigator on the left.
fn build_topbar(ctx: &Rc<WindowCtx>) -> Rc<TopBar> {
    let back = gtk4::Button::from_icon_name("go-previous-symbolic");
    back.set_tooltip_text(Some("Back (Alt+Left)"));
    back.set_sensitive(false);
    let forward = gtk4::Button::from_icon_name("go-next-symbolic");
    forward.set_tooltip_text(Some("Forward (Alt+Right)"));
    forward.set_sensitive(false);
    let title = gtk4::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .build();
    title.add_css_class("moremaid-topbar-title");

    let widget = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Start)
        .visible(false)
        .build();
    widget.add_css_class("moremaid-topbar");
    widget.append(&back);
    widget.append(&forward);
    widget.append(&title);

    {
        let ctx = ctx.clone();
        back.connect_clicked(move |_| go_history(&ctx, -1));
    }
    {
        let ctx = ctx.clone();
        forward.connect_clicked(move |_| go_history(&ctx, 1));
    }

    Rc::new(TopBar { widget, back, forward, title })
}

fn parse_headings_message(s: &str) -> Option<Vec<headings::Heading>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let hs = v.get("headings")?.as_array()?;
    Some(
        hs.iter()
            .filter_map(|h| {
                Some(headings::Heading {
                    level: h.get("level")?.as_u64()? as u8,
                    text: h.get("text")?.as_str()?.to_string(),
                    id: h.get("id")?.as_str()?.to_string(),
                })
            })
            .collect(),
    )
}

fn scroll_to_anchor(ctx: &Rc<WindowCtx>, id: &str) {
    // Clearing first makes a repeat click on the same heading jump again.
    let js = format!(
        "location.hash = ''; location.hash = '#' + {};",
        html::json_str(id)
    );
    ctx.webview
        .evaluate_javascript(&js, None, None, None::<&gio::Cancellable>, |_| {});
}

thread_local! {
    static CHROME_CSS: gtk4::CssProvider = {
        let provider = gtk4::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        provider
    };
}

/// Style the little GTK chrome there is (the Navigator) from the same
/// palette as the document, with the body face (§6.3, §6.4). One provider,
/// reloaded in place on retheme.
fn apply_chrome_css(palette: &theme::Palette, fonts: &config::Fonts) {
    let css = format!(
        r#"
window {{ background-color: {bg}; color: {fg}; }}
.moremaid-sidebar {{ background-color: {bg}; color: {fg}; font-family: {font_body}; border-right: 1px solid {muted}; }}
.moremaid-sidebar listview, .moremaid-sidebar listview row {{ background: transparent; }}
.moremaid-sidebar listview row:selected {{ background-color: {selection}; }}
.moremaid-sidebar listview row:hover {{ background-color: {selection}; }}
.moremaid-heading-row {{ color: {dim}; font-size: 90%; }}
paned > separator {{ background-color: {muted}; min-width: 1px; }}
.moremaid-overlay {{ background-color: {raised}; color: {fg}; border: 1px solid {muted}; border-radius: 8px; padding: 10px; }}
.moremaid-overlay entry {{ background-color: {bg}; color: {fg}; caret-color: {fg}; border: 1px solid {muted}; }}
.moremaid-overlay entry selection {{ background-color: {selection}; }}
.moremaid-overlay listview, .moremaid-overlay listview row {{ background: transparent; }}
.moremaid-overlay listview row:selected {{ background-color: {selection}; }}
.moremaid-overlay-mode {{ color: {dim}; font-size: 90%; }}
.moremaid-overlay-snippet {{ color: {dim}; font-size: 90%; font-family: monospace; }}
.moremaid-topbar {{ background-color: {raised}; color: {fg}; border-bottom: 1px solid {muted}; padding: 4px 8px; font-family: {font_body}; }}
.moremaid-topbar button {{ background: transparent; border: none; box-shadow: none; color: {fg}; min-width: 28px; min-height: 28px; }}
.moremaid-topbar button:disabled {{ color: {muted}; }}
.moremaid-topbar button:hover {{ background-color: {selection}; }}
.moremaid-topbar-title {{ color: {dim}; font-size: 95%; }}
"#,
        font_body = fonts.body_stack,
        bg = palette.get("background"),
        fg = palette.get("foreground"),
        selection = palette.get("selection"),
        dim = palette.get("dark_foreground"),
        muted = palette.get("muted"),
        raised = if palette.dark {
            palette.get("lighter_background")
        } else {
            palette.get("darker_background")
        },
    );
    CHROME_CSS.with(|provider| provider.load_from_string(&css));
}

/// Directory view: the auto-index page (§8 empty states included).
fn load_auto_index(ctx: &Rc<WindowCtx>, dir: &Path) {
    ctx.pending_anchor.replace(None);
    ctx.load_seq.set(ctx.load_seq.get() + 1);
    ctx.load_complete.set(false);
    // a directory listing is not a watched document
    ctx.doc_watcher.replace(None);
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let is_dir = meta.is_dir();
            if !is_dir && !langmap::is_markdown(&name) && !langmap::is_html(&name) {
                continue;
            }
            let modified_epoch = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push(html::IndexEntry {
                href: doc_uri(&entry.path()),
                name,
                is_dir,
                size: meta.len(),
                modified_epoch,
            });
        }
    }
    // Newest first — the same default setupAutoIndexSort applies, so the
    // table doesn't visibly reshuffle a frame after first paint.
    entries.sort_by_key(|e| std::cmp::Reverse(e.modified_epoch));

    let title = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.display().to_string());
    let parent_href = dir.parent().map(doc_uri);
    let page = html::auto_index_page(
        &ctx.web_dir,
        &ctx.palette.borrow(),
        &ctx.fonts,
        &title,
        &entries,
        parent_href.as_deref(),
        ctx.load_seq.get(),
    );
    let base = doc_base_uri(dir);
    ctx.doc_path.replace(None);
    ctx.base_uri.replace(base.clone());
    ctx.window.set_title(Some(&format!("{title} — Moremaid")));
    ctx.webview.load_html(&page, Some(&base));
    record_history(ctx, HistoryEntry::Dir(dir.to_path_buf()));
}

fn doc_uri(path: &Path) -> String {
    let escaped = glib::Uri::escape_string(&path.to_string_lossy(), Some("/"), true);
    format!("moremaid://doc{escaped}")
}

fn adw_dark_hint() -> bool {
    if libadwaita::is_initialized() || libadwaita::init().is_ok() {
        libadwaita::StyleManager::default().is_dark()
    } else {
        true
    }
}

/// Register the moremaid:// scheme once per process. Serves the web assets
/// (`moremaid://assets/...`) and local files (`moremaid://doc/<abs path>`),
/// which is what keeps absolute image paths in documents from terminating
/// the web process, lets vendored JS load offline, and — marked secure —
/// unblocks navigator.clipboard for the copy buttons (§5).
fn ensure_uri_scheme(web_dir: &Path) {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    let web_dir = web_dir.to_path_buf();
    REGISTERED.call_once(move || {
        let context = webkit6::WebContext::default().expect("WebContext");
        if let Some(sm) = context.security_manager() {
            sm.register_uri_scheme_as_secure("moremaid");
        }
        context.register_uri_scheme("moremaid", move |request| {
            serve_scheme_request(&web_dir, request);
        });
    });
}

fn serve_scheme_request(web_dir: &Path, request: &webkit6::URISchemeRequest) {
    let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
    let rest = uri.strip_prefix("moremaid://").unwrap_or("");
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let decoded = glib::Uri::unescape_string(path, None)
        .map(|g| g.to_string())
        .unwrap_or_else(|| path.to_string());

    let fs_path = match host {
        "assets" => {
            let joined = web_dir.join(&decoded);
            // no escaping the assets root via dot segments
            match joined.canonicalize() {
                Ok(p) if p.starts_with(web_dir) => p,
                _ => return finish_not_found(request, &uri),
            }
        }
        "doc" => PathBuf::from(format!("/{decoded}")),
        _ => return finish_not_found(request, &uri),
    };

    match std::fs::read(&fs_path) {
        Ok(data) => {
            let (ctype, _) = gio::functions::content_type_guess(
                Some(std::path::Path::new(
                    fs_path.file_name().unwrap_or_default(),
                )),
                data.as_slice(),
            );
            let mime = gio::functions::content_type_get_mime_type(&ctype)
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            let len = data.len() as i64;
            let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(data));
            request.finish(&stream, len, Some(&mime));
        }
        Err(_) => finish_not_found(request, &uri),
    }
}

fn finish_not_found(request: &webkit6::URISchemeRequest, uri: &str) {
    request.finish_error(&mut glib::Error::new(
        gio::IOErrorEnum::NotFound,
        &format!("not found: {uri}"),
    ));
}

fn load_path(ctx: &Rc<WindowCtx>, path: &Path) -> bool {
    // any navigation supersedes a pending heading jump
    ctx.pending_anchor.replace(None);
    ctx.load_seq.set(ctx.load_seq.get() + 1);
    ctx.load_complete.set(false);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("moremaid: {}: {e}", path.display());
            return false;
        }
    };

    let page = if langmap::is_markdown(&file_name) && !ctx.view_source.get() {
        html::markdown_page(
            &ctx.web_dir,
            &ctx.palette.borrow(),
            &ctx.fonts,
            &file_name,
            &content,
            ctx.force_full.get(),
            ctx.load_seq.get(),
        )
    } else if langmap::is_html(&file_name) && !ctx.view_source.get() {
        // raw HTML renders as a document (§7 htmlPage) — the base URI makes
        // its relative assets resolve over the scheme. These pages carry no
        // moremaid bridge, so anchors resolve on LoadEvent::Finished instead
        // of loadComplete. Like the macOS app, the document is trusted: it
        // is the user's own file.
        content
    } else {
        html::code_page(&ctx.web_dir, &ctx.palette.borrow(), &ctx.fonts, &file_name, &content, ctx.load_seq.get())
    };

    let dir = path.parent().unwrap_or_else(|| Path::new("/"));
    let base = doc_base_uri(dir);
    ctx.doc_path.replace(Some(path.to_path_buf()));
    ctx.base_uri.replace(base.clone());
    ctx.window.set_title(Some(&format!("{file_name} — Moremaid")));
    ctx.webview.load_html(&page, Some(&base));
    arm_doc_watcher(ctx);
    record_history(ctx, HistoryEntry::File(path.to_path_buf()));
    true
}

fn load_stdin(ctx: &Rc<WindowCtx>, src: String) {
    ctx.load_seq.set(ctx.load_seq.get() + 1);
    ctx.load_complete.set(false);
    // Base path is the CWD so relative links and images resolve from where
    // the command ran (§8). No live reload — there is nothing to watch.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let base = doc_base_uri(&cwd);
    let page = if ctx.view_source.get() {
        // "stdin.md" only steers Prism's language pick; the window title
        // stays "(stdin)"
        html::code_page(&ctx.web_dir, &ctx.palette.borrow(), &ctx.fonts, "stdin.md", &src, ctx.load_seq.get())
    } else {
        html::markdown_page(&ctx.web_dir, &ctx.palette.borrow(), &ctx.fonts, "(stdin)", &src, ctx.force_full.get(), ctx.load_seq.get())
    };
    ctx.stdin_src.replace(Some(src));
    ctx.base_uri.replace(base.clone());
    ctx.window.set_title(Some("(stdin) — Moremaid"));
    ctx.webview.load_html(&page, Some(&base));
}

fn doc_base_uri(dir: &Path) -> String {
    let escaped = glib::Uri::escape_string(&dir.to_string_lossy(), Some("/"), true);
    format!("moremaid://doc{escaped}/")
}

/// Link interception (§5): internal text files navigate in-app, external
/// links go to the system browser, Ctrl+click / middle-click / Shift+click
/// open a new window.
fn connect_link_policy(ctx: &Rc<WindowCtx>, _app: &gtk4::Application) {
    let ctx = ctx.clone();
    ctx.webview.clone().connect_decide_policy(move |_, decision, decision_type| {
        if decision_type != webkit6::PolicyDecisionType::NavigationAction {
            return false;
        }
        let Some(nav_decision) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
        else {
            return false;
        };
        let Some(action) = nav_decision.navigation_action() else {
            return false;
        };
        if action.navigation_type() != webkit6::NavigationType::LinkClicked {
            return false;
        }
        let Some(uri) = action.request().and_then(|r| r.uri()) else {
            return false;
        };
        let uri = uri.to_string();

        if uri.starts_with("http://") || uri.starts_with("https://") {
            decision.ignore();
            if let Err(e) = open::that_detached(&uri) {
                eprintln!("moremaid: failed to open {uri}: {e}");
            }
            return true;
        }

        if let Some(rest) = uri.strip_prefix("moremaid://doc") {
            let fragment = rest.split_once('#').map(|(_, f)| f.to_string());
            let without_fragment = rest.split('#').next().unwrap_or(rest);
            // same-document anchor → let WebKit scroll in place
            let base = ctx.base_uri.borrow();
            if format!("moremaid://doc{without_fragment}") == *base {
                return false;
            }
            drop(base);
            let path = glib::Uri::unescape_string(without_fragment, None)
                .map(|g| PathBuf::from(g.to_string()))
                .unwrap_or_else(|| PathBuf::from(without_fragment));
            // Rust wart (§5): modifiers() is a bare u32, not a typed
            // gdk::ModifierType. Compare against bits().
            let modifiers = action.modifiers();
            let button = action.mouse_button();
            let new_window = button == 2
                || modifiers & gdk::ModifierType::CONTROL_MASK.bits() != 0
                || modifiers & gdk::ModifierType::SHIFT_MASK.bits() != 0;
            if path.is_dir() {
                decision.ignore();
                if new_window {
                    spawn_window_for(&path);
                } else {
                    load_auto_index(&ctx, &path);
                }
                return true;
            }
            if path.is_file() && is_text_file(&path) {
                decision.ignore();
                if new_window {
                    spawn_window_for(&path);
                } else if load_path(&ctx, &path) {
                    // a cross-file anchor link ("other.md#section") keeps
                    // its fragment: jump once the target page completes
                    if let Some(fragment) = fragment {
                        if !fragment.is_empty() {
                            ctx.pending_anchor.replace(Some(fragment));
                        }
                    }
                }
                return true;
            }
        }
        false
    });
}

fn is_text_file(path: &Path) -> bool {
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let mut buf = [0u8; 8192];
            match f.read(&mut buf) {
                Ok(n) => !buf[..n].contains(&0),
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// New windows are new processes — no shared state, crashes isolated (§6.1).
fn spawn_window_for(path: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(path).spawn();
    }
}

fn open_diagram_window(app: &gtk4::Application, ctx: &Rc<WindowCtx>, definition: &str) {
    let webview = webkit6::WebView::new();
    if let Ok(bg) = ctx.palette.borrow().get("background").parse::<gdk::RGBA>() {
        webview.set_background_color(&bg);
    }
    // Under a tiler this window will tile; the README documents a float rule
    // keyed on its title (§6.1, §10).
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(900)
        .default_height(700)
        .title("Mermaid Diagram — Moremaid")
        .child(&webview)
        .build();
    let page = html::diagram_page(&ctx.palette.borrow(), &ctx.fonts, definition);
    webview.load_html(&page, Some("moremaid://assets/"));
    window.present();
}

fn install_actions(app: &gtk4::Application, ctx: &Rc<WindowCtx>) {
    let make = |name: &str, ctx: &Rc<WindowCtx>, f: fn(&Rc<WindowCtx>)| {
        let action = gio::SimpleAction::new(name, None);
        let ctx = ctx.clone();
        action.connect_activate(move |_, _| f(&ctx));
        action
    };

    let zoom_in = make("zoom-in", ctx, |c| {
        c.app_zoom.set((c.app_zoom.get() * 1.1).min(5.0));
        apply_zoom(c);
    });
    let zoom_out = make("zoom-out", ctx, |c| {
        c.app_zoom.set((c.app_zoom.get() / 1.1).max(0.25));
        apply_zoom(c);
    });
    // Ctrl+0 resets the app's own factor while still respecting the
    // system text scale (§6.4)
    let zoom_reset = make("zoom-reset", ctx, |c| {
        c.app_zoom.set(1.0);
        apply_zoom(c);
    });
    let force_render = make("force-render", ctx, |c| {
        // Force full render of a large document (§8); populates the diagram
        // cache normally.
        c.force_full.set(true);
        let path = c.doc_path.borrow().clone();
        if let Some(path) = path {
            load_path(c, &path);
        } else {
            let src = c.stdin_src.borrow().clone();
            if let Some(src) = src {
                load_stdin(c, src);
            }
        }
    });

    // Ctrl+B pins the Navigator open; unpinning returns it to auto-hide
    let toggle_sidebar = make("toggle-sidebar", ctx, |c| {
        if let Some(sb) = &*c.sidebar.borrow() {
            let pinned = !c.sidebar_pinned.get();
            c.sidebar_pinned.set(pinned);
            sb.widget.set_visible(pinned);
        }
    });
    let quick_open = make("quick-open", ctx, |c| {
        if let Some(overlay) = &*c.overlay.borrow() {
            overlay.open(overlay::Mode::Filename);
        }
    });
    let find_in_files = make("find-in-files", ctx, |c| {
        if let Some(overlay) = &*c.overlay.borrow() {
            overlay.open(overlay::Mode::Content);
        }
    });
    let show_shortcuts = make("show-shortcuts", ctx, |c| {
        show_shortcuts_dialog(&c.window);
    });
    let go_back = make("go-back", ctx, |c| go_history(c, -1));
    let go_forward = make("go-forward", ctx, |c| go_history(c, 1));
    // Ctrl+M: rendered document ↔ raw markdown source
    let toggle_source = make("toggle-source", ctx, |c| {
        c.view_source.set(!c.view_source.get());
        let path = c.doc_path.borrow().clone();
        if let Some(path) = path {
            load_path(c, &path);
        } else {
            let src = c.stdin_src.borrow().clone();
            if let Some(src) = src {
                load_stdin(c, src);
            }
        }
    });
    let new_window = make("new-window", ctx, |c| {
        let target = c
            .root_dir
            .borrow()
            .clone()
            .or_else(|| c.doc_path.borrow().clone());
        if let Some(target) = target {
            spawn_window_for(&target);
        }
    });

    for a in [
        &zoom_in, &zoom_out, &zoom_reset, &force_render, &toggle_sidebar,
        &quick_open, &find_in_files, &new_window, &show_shortcuts,
        &go_back, &go_forward, &toggle_source,
    ] {
        ctx.window.add_action(a);
    }

    // mouse back/forward buttons (8/9), captured before the WebView
    {
        let ctx_click = ctx.clone();
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(0);
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
        gesture.connect_pressed(move |gesture, _, _, _| {
            match gesture.current_button() {
                8 => go_history(&ctx_click, -1),
                9 => go_history(&ctx_click, 1),
                _ => return,
            }
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        });
        ctx.window.add_controller(gesture);
    }
    // Ctrl for the app; Super is the compositor's and must never be bound (§6.5).
    app.set_accels_for_action("win.zoom-in", &["<Control>plus", "<Control>equal", "<Control>KP_Add"]);
    app.set_accels_for_action("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]);
    app.set_accels_for_action("win.zoom-reset", &["<Control>0", "<Control>KP_0"]);
    app.set_accels_for_action("win.force-render", &["<Control><Shift>r"]);
    app.set_accels_for_action("win.toggle-sidebar", &["<Control>b"]);
    app.set_accels_for_action("win.quick-open", &["<Control>p"]);
    app.set_accels_for_action("win.find-in-files", &["<Control><Shift>f"]);
    app.set_accels_for_action("win.new-window", &["<Control>n"]);
    // `?` (in the vim-key controller) is the canonical binding; F1 is the
    // conventional fallback and works even where bare keys are consumed.
    app.set_accels_for_action("win.show-shortcuts", &["F1"]);
    app.set_accels_for_action("win.go-back", &["<Alt>Left"]);
    app.set_accels_for_action("win.go-forward", &["<Alt>Right"]);
    app.set_accels_for_action("win.toggle-source", &["<Control>m"]);
}
