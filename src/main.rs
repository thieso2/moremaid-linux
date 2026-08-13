//! Moremaid — a markdown reader for Omarchy/Hyprland.
//!
//! One window per invocation, no in-app tabs, no session restore (§6.1).
//! The compositor owns window management; the app renders documents.

mod html;
mod langmap;
mod theme;

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
    Stdin(String),
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
            if path.is_dir() {
                return Err(format!(
                    "{arg} is a directory — directory browsing lands in Milestone 2"
                ));
            }
            let path = path
                .canonicalize()
                .map_err(|e| format!("{arg}: {e}"))?;
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
                Err("usage: moremaid <file>   (or pipe markdown on stdin)".into())
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
    palette: theme::Palette,
    web_dir: PathBuf,
    doc_path: RefCell<Option<PathBuf>>,
    stdin_src: RefCell<Option<String>>,
    base_uri: RefCell<String>,
    force_full: Cell<bool>,
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
        .child(&webview)
        .build();

    let ctx = Rc::new(WindowCtx {
        webview: webview.clone(),
        window: window.clone(),
        palette,
        web_dir,
        doc_path: RefCell::new(None),
        stdin_src: RefCell::new(None),
        base_uri: RefCell::new(String::new()),
        force_full: Cell::new(false),
    });

    // JS → Rust bridge. Payloads are JSON strings (or, for openDiagram, the
    // raw diagram definition) — see web/js/page.js.
    {
        let started = started;
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
            // headings arrive here too — consumed by the Navigator in M2.
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
    trace(started, "webview + window built");

    {
        let started = started;
        webview.connect_load_changed(move |_, event| {
            if event == webkit6::LoadEvent::Finished {
                trace(started, "webkit load finished");
            }
        });
    }

    match target {
        Target::File(path) => load_path(&ctx, &path),
        Target::Stdin(src) => load_stdin(&ctx, src),
    }
    trace(started, "load_html issued");

    window.present();
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

fn load_path(ctx: &Rc<WindowCtx>, path: &Path) {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("moremaid: {}: {e}", path.display());
            return;
        }
    };

    let page = if langmap::is_markdown(&file_name) {
        html::markdown_page(
            &ctx.web_dir,
            &ctx.palette,
            &file_name,
            &content,
            ctx.force_full.get(),
        )
    } else {
        html::code_page(&ctx.web_dir, &ctx.palette, &file_name, &content)
    };

    let dir = path.parent().unwrap_or_else(|| Path::new("/"));
    let base = doc_base_uri(dir);
    ctx.doc_path.replace(Some(path.to_path_buf()));
    ctx.base_uri.replace(base.clone());
    ctx.window.set_title(Some(&format!("{file_name} — Moremaid")));
    ctx.webview.load_html(&page, Some(&base));
}

fn load_stdin(ctx: &Rc<WindowCtx>, src: String) {
    // Base path is the CWD so relative links and images resolve from where
    // the command ran (§8). No live reload — there is nothing to watch.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let base = doc_base_uri(&cwd);
    let page = html::markdown_page(&ctx.web_dir, &ctx.palette, "(stdin)", &src, ctx.force_full.get());
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
            if path.is_file() && is_text_file(&path) {
                decision.ignore();
                // Rust wart (§5): modifiers() is a bare u32, not a typed
                // gdk::ModifierType. Compare against bits().
                let modifiers = action.modifiers();
                let button = action.mouse_button();
                let new_window = button == 2
                    || modifiers & gdk::ModifierType::CONTROL_MASK.bits() != 0
                    || modifiers & gdk::ModifierType::SHIFT_MASK.bits() != 0;
                if new_window {
                    spawn_window_for(&path);
                } else {
                    load_path(&ctx, &path);
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
    if let Ok(bg) = ctx.palette.get("background").parse::<gdk::RGBA>() {
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
    let page = html::diagram_page(&ctx.palette, definition);
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
        c.webview.set_zoom_level((c.webview.zoom_level() * 1.1).min(5.0));
    });
    let zoom_out = make("zoom-out", ctx, |c| {
        c.webview.set_zoom_level((c.webview.zoom_level() / 1.1).max(0.25));
    });
    let zoom_reset = make("zoom-reset", ctx, |c| c.webview.set_zoom_level(1.0));
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

    for a in [&zoom_in, &zoom_out, &zoom_reset, &force_render] {
        ctx.window.add_action(a);
    }
    // Ctrl for the app; Super is the compositor's and must never be bound (§6.5).
    app.set_accels_for_action("win.zoom-in", &["<Control>plus", "<Control>equal", "<Control>KP_Add"]);
    app.set_accels_for_action("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]);
    app.set_accels_for_action("win.zoom-reset", &["<Control>0", "<Control>KP_0"]);
    app.set_accels_for_action("win.force-render", &["<Control><Shift>r"]);
}
