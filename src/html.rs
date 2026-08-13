//! HTML page assembly: read the web assets, substitute a handful of values,
//! hand the result to `load_html`. The rendering layer itself (CSS/JS) lives
//! as data files on disk (§4) — served over `moremaid://assets/`, with the
//! document stylesheet inlined so there is never a flash of unstyled content
//! (§9.5). Skeleton ported from HTMLGenerator.swift @ a3ab7fd.

use crate::theme::Palette;
use gtk4::glib;
use std::path::{Path, PathBuf};

/// Above these, render plain (no Mermaid, no Prism) behind a banner (§8).
/// The diagram count is the real trigger.
pub const PLAIN_SIZE_LIMIT: usize = 5 * 1024 * 1024;
pub const PLAIN_DIAGRAM_LIMIT: usize = 50;

/// Locate the web assets directory (§4): user override, then the package.
pub fn web_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("MOREMAID_WEB_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let xdg_data = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"));
    [
        xdg_data.join("moremaid/web"),
        PathBuf::from("/usr/share/moremaid/web"),
        // dev builds: the repo's web/ next to Cargo.toml
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

fn asset(web: &Path, rel: &str) -> String {
    std::fs::read_to_string(web.join(rel)).unwrap_or_else(|e| {
        eprintln!("moremaid: missing web asset {rel}: {e}");
        String::new()
    })
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// JSON string literal, safe for embedding inside a <script> block
/// ('<' escaped so a literal "</script>" in the document can't end the block).
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn count_mermaid_fences(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            (t.starts_with("```") || t.starts_with("~~~"))
                && t.trim_start_matches(['`', '~']).trim().starts_with("mermaid")
        })
        .count()
}

fn head_common(web: &Path, palette: &Palette, title: &str) -> String {
    format!(
        r#"<meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <style>
{palette_css}
{base_css}
{prism_css}
    </style>"#,
        title = html_escape(title),
        palette_css = palette.css_block(16),
        base_css = asset(web, "css/base.css"),
        prism_css = asset(web, "css/prism-theme.css"),
    )
}

pub fn markdown_page(
    web: &Path,
    palette: &Palette,
    title: &str,
    markdown: &str,
    force_full: bool,
) -> String {
    let plain = !force_full
        && (markdown.len() > PLAIN_SIZE_LIMIT
            || count_mermaid_fences(markdown) > PLAIN_DIAGRAM_LIMIT);
    let banner = if plain {
        r#"<div class="moremaid-banner">Large document &mdash; rendered without diagrams or syntax highlighting. Press Ctrl+Shift+R to render fully.</div>"#
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en" data-theme="{mode}">
<head>
    {head}
    <script src="moremaid://assets/vendor/markdown-it.min.js"></script>
    <script src="moremaid://assets/vendor/markdown-it-task-lists.min.js"></script>
</head>
<body>
    {banner}<div class="container"><div id="content"></div></div>
    <script>
var __MOREMAID__ = {{ mermaidVars: {mermaid_vars}, plain: {plain} }};
var rawMarkdown = {markdown_json};
var documentTitle = {title_json};
{page_js}
    </script>
</body>
</html>"#,
        mode = if palette.dark { "dark" } else { "light" },
        head = head_common(web, palette, title),
        mermaid_vars = palette.mermaid_vars_json(),
        markdown_json = json_str(markdown),
        title_json = json_str(title),
        page_js = asset(web, "js/page.js"),
    )
}

pub fn code_page(web: &Path, palette: &Palette, file_name: &str, content: &str) -> String {
    let language = crate::langmap::language_for_file(file_name);
    format!(
        r#"<!DOCTYPE html>
<html lang="en" data-theme="{mode}">
<head>
    {head}
</head>
<body>
    <div class="container"><pre><code class="language-{language}">{content}</code></pre></div>
    <script>
var __MOREMAID__ = {{ mermaidVars: {mermaid_vars}, plain: false }};
var documentTitle = {title_json};
{page_js}
    </script>
</body>
</html>"#,
        mode = if palette.dark { "dark" } else { "light" },
        head = head_common(web, palette, file_name),
        mermaid_vars = palette.mermaid_vars_json(),
        content = html_escape(content),
        title_json = json_str(file_name),
        page_js = asset(web, "js/page.js"),
    )
}

pub struct IndexEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_epoch: i64,
    pub href: String,
}

/// Directory listing shown when a directory is opened — the auto-index page.
/// Table markup matches what setupAutoIndexSort in page.js expects.
pub fn auto_index_page(
    web: &Path,
    palette: &Palette,
    title: &str,
    entries: &[IndexEntry],
    parent_href: Option<&str>,
) -> String {
    let body = if entries.is_empty() {
        // Empty states get a message, never a blank window (§8).
        r#"<p class="file-info">No markdown files here.</p>"#.to_string()
    } else {
        let rows: String = entries
            .iter()
            .map(|e| {
                let display_name = if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name.clone()
                };
                let size_cell = if e.is_dir {
                    String::new()
                } else {
                    human_size(e.size)
                };
                let date_cell = glib::DateTime::from_unix_local(e.modified_epoch)
                    .ok()
                    .and_then(|d| d.format("%Y-%m-%d %H:%M").ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                format!(
                    r#"<tr data-name="{name}" data-size="{size}" data-date="{date}"><td><a href="{href}">{display}</a></td><td class="ai-size">{size_cell}</td><td class="ai-date">{date_cell}</td></tr>"#,
                    name = html_escape(&e.name),
                    // directories sort together under the Size column
                    // instead of interleaving at their inode size
                    size = if e.is_dir { 0 } else { e.size },
                    date = e.modified_epoch,
                    href = html_escape(&e.href),
                    display = html_escape(&display_name),
                )
            })
            .collect();
        format!(
            r#"<table class="auto-index">
<thead><tr><th class="ai-sortable" data-sort="name">Name</th><th class="ai-sortable" data-sort="size">Size</th><th class="ai-sortable" data-sort="modified">Modified</th></tr></thead>
<tbody>{rows}</tbody>
</table>"#
        )
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en" data-theme="{mode}">
<head>
    {head}
</head>
<body>
    <div class="container">{nav}<h1>{title_h}</h1>{body}</div>
    <script>
var __MOREMAID__ = {{ mermaidVars: {mermaid_vars}, plain: false }};
var documentTitle = {title_json};
{page_js}
    </script>
</body>
</html>"#,
        mode = if palette.dark { "dark" } else { "light" },
        head = head_common(web, palette, title),
        // lives outside the sortable table so it can't be re-ordered away
        nav = parent_href
            .map(|href| format!(r#"<div class="nav-bar"><a href="{}">&uarr; ..</a></div>"#, html_escape(href)))
            .unwrap_or_default(),
        title_h = html_escape(title),
        mermaid_vars = palette.mermaid_vars_json(),
        title_json = json_str(title),
        page_js = asset(web, "js/page.js"),
    )
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// The standalone diagram viewer (zoom dropdown, Ctrl +/− zoom, pan).
/// Ported from HTMLGenerator.diagramPage @ a3ab7fd; palette-fed, macOS key
/// hints swapped for their Linux equivalents.
pub fn diagram_page(palette: &Palette, definition: &str) -> String {
    let raised = if palette.dark { palette.get("lighter_background") } else { palette.get("darker_background") };
    format!(
        r#"<!DOCTYPE html>
<html lang="en" data-theme="{mode}">
<head>
    <meta charset="UTF-8">
    <title>Mermaid Diagram</title>
    <script src="moremaid://assets/vendor/mermaid.min.js"></script>
    <style>
    html, body {{
        margin: 0; height: 100%; overflow: hidden;
        background: {bg};
        font-family: {font_body};
    }}
    #viewport {{ position: absolute; inset: 0; overflow: hidden; cursor: grab; }}
    #viewport.panning {{ cursor: grabbing; }}
    #stage {{ position: absolute; top: 0; left: 0; transform-origin: 0 0; }}
    #stage svg {{ display: block; }}
    #toolbar {{
        position: fixed; top: 12px; right: 12px; z-index: 10;
        display: flex; align-items: center; gap: 4px;
        background: {raised}; color: {fg};
        border: 1px solid {muted}; border-radius: 8px;
        padding: 4px 6px; box-shadow: 0 2px 8px rgba(0,0,0,0.25);
        -webkit-user-select: none; user-select: none;
    }}
    #toolbar button {{
        background: transparent; color: inherit; border: none; border-radius: 5px;
        width: 24px; height: 24px; font-size: 15px; line-height: 1; cursor: pointer;
    }}
    #toolbar button:hover {{ background: {muted}; }}
    #zoomSelect {{
        background: transparent; color: inherit;
        border: 1px solid {muted}; border-radius: 5px;
        font-size: 12px; padding: 2px 4px; cursor: pointer;
    }}
    #error {{
        color: {red}; padding: 20px; margin: 40px auto; max-width: 600px;
        background: {raised}; border-radius: 5px; font-size: 14px;
        white-space: pre-wrap;
    }}
    </style>
</head>
<body>
    <div id="viewport"><div id="stage"></div></div>
    <div id="toolbar">
        <button id="zoomOutBtn" title="Zoom Out (-)">&minus;</button>
        <select id="zoomSelect" title="Zoom">
            <option value="fit">Fit</option>
            <option id="zoomCustom" hidden></option>
            <option value="50">50%</option>
            <option value="75">75%</option>
            <option value="100">100%</option>
            <option value="125">125%</option>
            <option value="150">150%</option>
            <option value="200">200%</option>
            <option value="300">300%</option>
            <option value="400">400%</option>
        </select>
        <button id="zoomInBtn" title="Zoom In (+)">+</button>
    </div>
    <script>
    var graphDefinition = {definition_json};
    mermaid.initialize({{ startOnLoad: false, theme: 'base', themeVariables: {mermaid_vars} }});

    var viewport = document.getElementById('viewport');
    var stage = document.getElementById('stage');
    var zoomSelect = document.getElementById('zoomSelect');
    var zoomCustom = document.getElementById('zoomCustom');
    var MIN_SCALE = 0.1, MAX_SCALE = 8, STEP = 1.25;
    var scale = 1, tx = 0, ty = 0;
    var contentW = 0, contentH = 0;
    var fitMode = true;

    function applyTransform() {{
        stage.style.transform = 'translate(' + tx + 'px,' + ty + 'px) scale(' + scale + ')';
        updateZoomSelect();
    }}

    function updateZoomSelect() {{
        if (fitMode) {{ zoomCustom.hidden = true; zoomSelect.value = 'fit'; return; }}
        var pct = String(Math.round(scale * 100));
        var presets = ['50','75','100','125','150','200','300','400'];
        if (presets.indexOf(pct) >= 0) {{
            zoomCustom.hidden = true;
        }} else {{
            zoomCustom.hidden = false;
            zoomCustom.value = pct;
            zoomCustom.textContent = pct + '%';
        }}
        zoomSelect.value = pct;
    }}

    function setScale(newScale, cx, cy) {{
        newScale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, newScale));
        if (cx === undefined) {{ cx = viewport.clientWidth / 2; cy = viewport.clientHeight / 2; }}
        var k = newScale / scale;
        tx = cx - (cx - tx) * k;
        ty = cy - (cy - ty) * k;
        scale = newScale;
        fitMode = false;
        applyTransform();
    }}

    function fitToWindow() {{
        if (!contentW || !contentH) return;
        var pad = 40;
        var vw = Math.max(50, viewport.clientWidth - pad);
        var vh = Math.max(50, viewport.clientHeight - pad);
        scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, Math.min(vw / contentW, vh / contentH)));
        tx = (viewport.clientWidth - contentW * scale) / 2;
        ty = (viewport.clientHeight - contentH * scale) / 2;
        fitMode = true;
        applyTransform();
    }}

    window.moremaidDiagramZoomIn = function() {{ setScale(scale * STEP); }};
    window.moremaidDiagramZoomOut = function() {{ setScale(scale / STEP); }};
    window.moremaidDiagramZoomReset = function() {{ setScale(1); }};

    document.getElementById('zoomInBtn').onclick = function() {{ moremaidDiagramZoomIn(); }};
    document.getElementById('zoomOutBtn').onclick = function() {{ moremaidDiagramZoomOut(); }};
    zoomSelect.addEventListener('change', function() {{
        if (zoomSelect.value === 'fit') fitToWindow();
        else setScale(parseFloat(zoomSelect.value) / 100);
        zoomSelect.blur();
    }});

    var panning = false, lastX = 0, lastY = 0;
    viewport.addEventListener('mousedown', function(e) {{
        if (e.button !== 0) return;
        panning = true; lastX = e.clientX; lastY = e.clientY;
        viewport.classList.add('panning');
        e.preventDefault();
    }});
    window.addEventListener('mousemove', function(e) {{
        if (!panning) return;
        tx += e.clientX - lastX; ty += e.clientY - lastY;
        lastX = e.clientX; lastY = e.clientY;
        fitMode = false;
        applyTransform();
    }});
    window.addEventListener('mouseup', function() {{
        panning = false;
        viewport.classList.remove('panning');
    }});

    viewport.addEventListener('wheel', function(e) {{
        e.preventDefault();
        if (e.ctrlKey) {{
            setScale(scale * Math.pow(1.01, -e.deltaY), e.clientX, e.clientY);
        }} else {{
            tx -= e.deltaX; ty -= e.deltaY;
            fitMode = false;
            applyTransform();
        }}
    }}, {{ passive: false }});

    document.addEventListener('keydown', function(e) {{
        if (e.target === zoomSelect) return;
        if (e.key === '+' || e.key === '=') {{ moremaidDiagramZoomIn(); e.preventDefault(); }}
        else if (e.key === '-') {{ moremaidDiagramZoomOut(); e.preventDefault(); }}
        else if (e.key === '0') {{ fitToWindow(); e.preventDefault(); }}
        else if (e.key === '1') {{ moremaidDiagramZoomReset(); e.preventDefault(); }}
        else if (e.key === 'ArrowLeft') {{ tx += 40; fitMode = false; applyTransform(); e.preventDefault(); }}
        else if (e.key === 'ArrowRight') {{ tx -= 40; fitMode = false; applyTransform(); e.preventDefault(); }}
        else if (e.key === 'ArrowUp') {{ ty += 40; fitMode = false; applyTransform(); e.preventDefault(); }}
        else if (e.key === 'ArrowDown') {{ ty -= 40; fitMode = false; applyTransform(); e.preventDefault(); }}
    }});

    window.addEventListener('resize', function() {{ if (fitMode) fitToWindow(); }});

    (async function() {{
        try {{
            var result = await mermaid.render('diagram', graphDefinition);
            stage.innerHTML = result.svg;
            var svg = stage.querySelector('svg');
            svg.style.maxWidth = 'none';
            var vb = svg.viewBox && svg.viewBox.baseVal;
            if (vb && vb.width && vb.height) {{
                contentW = vb.width; contentH = vb.height;
                svg.setAttribute('width', vb.width);
                svg.setAttribute('height', vb.height);
            }} else {{
                var r = svg.getBoundingClientRect();
                contentW = r.width; contentH = r.height;
            }}
            fitToWindow();
        }} catch (error) {{
            document.getElementById('toolbar').style.display = 'none';
            var div = document.createElement('div');
            div.id = 'error';
            div.textContent = 'Error rendering diagram: ' + error.message;
            document.body.appendChild(div);
        }}
    }})();
    </script>
</body>
</html>"#,
        mode = if palette.dark { "dark" } else { "light" },
        bg = palette.get("background"),
        fg = palette.get("foreground"),
        muted = palette.get("muted"),
        red = palette.get("red"),
        raised = raised,
        font_body = crate::theme::FONT_BODY,
        definition_json = json_str(definition),
        mermaid_vars = palette.mermaid_vars_json(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_fence_counting() {
        let md = "```mermaid\ngraph TD\n```\n\n```rust\nfn x() {}\n```\n\n~~~mermaid\npie\n~~~\n";
        assert_eq!(count_mermaid_fences(md), 2);
    }

    #[test]
    fn json_escaping_script_safe() {
        let s = json_str("a</script>\"b\"\n");
        assert!(!s.contains("</script>"));
        assert!(s.contains("\\u003c/script"));
        assert!(s.contains("\\\"b\\\""));
    }

    #[test]
    fn html_escaping() {
        assert_eq!(html_escape("<a & \"b\">"), "&lt;a &amp; &quot;b&quot;&gt;");
    }
}
